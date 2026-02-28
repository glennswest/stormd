use clap::Parser;
use std::future::IntoFuture;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info, warn};

use stormd::api;
use stormd::backup::BackupManager;
use stormd::config::Config;
use stormd::cron::CronScheduler;
use stormd::events::{EventBus, EventKind};
use stormd::logger::LogManager;
use stormd::stats::StatsCollector;
use stormd::supervisor::Supervisor;

#[derive(Parser)]
#[command(name = "stormd", version, about = "Container init system for scratch images")]
struct Cli {
    /// Path to configuration file
    #[arg(short, long, default_value = "/etc/stormd/config.toml")]
    config: PathBuf,
}

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,stormd=debug".parse().unwrap()),
        )
        .json()
        .init();

    let cli = Cli::parse();

    info!(config = %cli.config.display(), "stormd starting");

    // Load configuration
    let config = match Config::load(&cli.config) {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, path = %cli.config.display(), "failed to load config");
            std::process::exit(1);
        }
    };

    // Initialize components
    let event_bus = Arc::new(EventBus::new(
        config.events.clone(),
        config.general.name.clone(),
    ));

    // Connect event transports
    if config.events.enabled {
        if let Err(e) = event_bus.connect().await {
            error!(error = %e, "failed to connect event transport");
            std::process::exit(1);
        }
    }

    event_bus
        .emit_simple(EventKind::ContainerStarting, None)
        .await;

    let log_manager = match LogManager::new(
        config.general.log_dir.clone(),
        config.log.clone(),
    )
    .await
    {
        Ok(lm) => Arc::new(lm),
        Err(e) => {
            error!(error = %e, "failed to initialize log manager");
            std::process::exit(1);
        }
    };

    let supervisor = Arc::new(Supervisor::new(log_manager.clone(), event_bus.clone()));
    let cron_scheduler = Arc::new(CronScheduler::new(log_manager.clone(), event_bus.clone()));
    let stats = Arc::new(StatsCollector::new(config.general.name.clone()));
    let backup = Arc::new(BackupManager::new(config.backup.clone(), event_bus.clone()));

    // Start the supervisor exit handler loop
    let sup_exit = supervisor.clone();
    tokio::spawn(async move { sup_exit.run_exit_handler().await });

    // Register cron jobs
    if !config.cron.is_empty() {
        if let Err(e) = cron_scheduler.register_jobs(&config.cron).await {
            error!(error = %e, "failed to register cron jobs");
            std::process::exit(1);
        }
        let cron = cron_scheduler.clone();
        tokio::spawn(async move { cron.run().await });
    }

    // Start supervised processes
    let sup = supervisor.clone();
    let process_configs = config.process.clone();
    let start_handle = tokio::spawn(async move {
        if let Err(e) = sup.start_all(&process_configs).await {
            error!(error = %e, "failed to start processes");
        }
    });

    // Build and start API server
    let app_state = Arc::new(api::AppState {
        supervisor: supervisor.clone(),
        log_manager: log_manager.clone(),
        cron_scheduler: cron_scheduler.clone(),
        stats: stats.clone(),
        backup: backup.clone(),
        debug_enabled: config.debug.enabled,
        allow_signal: config.debug.allow_signal,
        allow_stdin: config.debug.allow_stdin,
    });

    let router = api::build_router(app_state);
    let bind_addr = config.api.bind.clone();

    let listener = match tokio::net::TcpListener::bind(&bind_addr).await {
        Ok(l) => l,
        Err(e) => {
            error!(error = %e, addr = %bind_addr, "failed to bind API server");
            std::process::exit(1);
        }
    };

    info!(addr = %bind_addr, "REST API listening");

    // Set up signal handlers
    let sup_shutdown = supervisor.clone();
    let cron_shutdown = cron_scheduler.clone();
    let event_bus_shutdown = event_bus.clone();
    let backup_shutdown = backup.clone();
    let log_dir = config.general.log_dir.clone();
    let backup_on_failure = config.backup.enabled && config.backup.on_failure;

    let shutdown_signal = async move {
        let ctrl_c = tokio::signal::ctrl_c();

        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM handler");
            tokio::select! {
                _ = ctrl_c => info!("received SIGINT"),
                _ = sigterm.recv() => info!("received SIGTERM"),
            }
        }

        #[cfg(not(unix))]
        {
            ctrl_c.await.expect("ctrl-c handler");
            info!("received SIGINT");
        }
    };

    // PID 1 zombie reaper (Linux only)
    #[cfg(target_os = "linux")]
    tokio::spawn(async {
        reap_zombies().await;
    });

    // Monitor loop — check for container failure
    let sup_monitor = supervisor.clone();
    let monitor_handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            if sup_monitor.has_failed().await {
                warn!("container failure detected");
                return;
            }
        }
    });

    // Wait for either: API shutdown signal, process startup, or container failure
    tokio::select! {
        _ = shutdown_signal => {
            info!("shutdown signal received — stopping");
        }
        _ = monitor_handle => {
            error!("container failure — initiating shutdown");
        }
        result = axum::serve(listener, router).into_future() => {
            if let Err(e) = result {
                error!(error = %e, "API server error");
            }
        }
    }

    // Graceful shutdown
    event_bus_shutdown
        .emit_simple(EventKind::ContainerStopping, None)
        .await;

    sup_shutdown.stop_all().await;
    cron_shutdown.shutdown();
    let _ = start_handle.await;

    // Backup logs on failure if configured
    if supervisor.has_failed().await && backup_on_failure {
        info!("backing up logs before exit");
        if let Err(e) = backup_shutdown.backup_logs(&log_dir).await {
            error!(error = %e, "log backup failed");
        }
    }

    info!("stormd shutdown complete");

    // Exit with error code if container failed
    if supervisor.has_failed().await {
        std::process::exit(1);
    }
}

/// PID 1 zombie reaper — required when running as init in a container.
#[cfg(target_os = "linux")]
async fn reap_zombies() {
    use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
    use nix::unistd::Pid;

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        loop {
            match waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)) {
                Ok(WaitStatus::StillAlive) | Err(_) => break,
                Ok(status) => {
                    tracing::trace!(?status, "reaped zombie process");
                }
            }
        }
    }
}
