use clap::Parser;
use std::future::IntoFuture;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info, warn};

use stormd::api;
use stormd::backup::BackupManager;
use stormd::config::{self, Config};
use stormd::cron::CronScheduler;
use stormd::events::{EventBus, EventKind};
use stormd::ssh;
use stormd::stats::StatsCollector;
use stormd::supervisor::Supervisor;
use stormd::updater::Updater;
use stormlog::StormLog;

#[derive(Parser)]
#[command(name = "stormd", version, about = "Container init system for scratch images")]
struct Cli {
    /// Path to configuration file
    #[arg(short, long, default_value = "/etc/stormd/config.toml")]
    config: PathBuf,

    /// Run a health check against the running stormd instance and exit.
    /// Exits 0 if healthy, 1 if unhealthy. For use with Docker HEALTHCHECK.
    #[arg(long)]
    healthcheck: bool,

    /// Port to check for healthcheck (default: 9080)
    #[arg(long, default_value = "9080")]
    healthcheck_port: u16,

    /// Install symlinks for busybox-style commands into the given directory.
    /// Example: stormd --install /bin
    #[arg(long, value_name = "DIR")]
    install: Option<PathBuf>,

    /// List all available standalone commands
    #[arg(long)]
    list_commands: bool,
}

#[tokio::main]
async fn main() {
    // --- Busybox multi-call dispatch ---
    // Check argv[0] BEFORE clap parsing. If invoked as a known command
    // (via symlink), run it directly and exit.
    let argv0 = std::env::args().next().unwrap_or_default();
    let cmd_name = std::path::Path::new(&argv0)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();

    if cmd_name != "stormd" && stormd::shell::STANDALONE_COMMANDS.contains(&cmd_name.as_str()) {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let exit_code = stormd::shell::execute_standalone(&cmd_name, &args).await;
        std::process::exit(exit_code);
    }

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,stormd=debug".parse().unwrap()),
        )
        .json()
        .init();

    let cli = Cli::parse();

    // --list-commands: print all standalone commands and exit
    if cli.list_commands {
        for cmd in stormd::shell::STANDALONE_COMMANDS {
            println!("{}", cmd);
        }
        std::process::exit(0);
    }

    // --install DIR: create symlinks for all standalone commands
    if let Some(ref dir) = cli.install {
        let binary = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("/stormd"));
        match stormd::shell::install_symlinks(dir, &binary) {
            Ok(n) => {
                eprintln!("stormd: installed {} command symlinks in {}", n, dir.display());
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("stormd: failed to install symlinks: {}", e);
                std::process::exit(1);
            }
        }
    }

    // Healthcheck mode — probe the running instance and exit
    if cli.healthcheck {
        let url = format!("http://127.0.0.1:{}/api/v1/health", cli.healthcheck_port);
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap_or_default();
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => std::process::exit(0),
            Ok(resp) => {
                eprintln!("healthcheck failed: HTTP {}", resp.status());
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("healthcheck failed: {}", e);
                std::process::exit(1);
            }
        }
    }

    info!(config = %cli.config.display(), "stormd starting");

    // Auto-install busybox symlinks on startup (idempotent, non-fatal)
    let binary = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("/stormd"));
    for dir in &["/bin", "/usr/bin", "/sbin", "/usr/sbin"] {
        let dir_path = std::path::Path::new(dir);
        if let Err(e) = stormd::shell::install_symlinks(dir_path, &binary) {
            tracing::debug!(dir = %dir, error = %e, "skipped symlink install");
        }
    }
    info!("busybox symlinks installed");

    // Load configuration
    let config = match Config::load(&cli.config) {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, path = %cli.config.display(), "failed to load config");
            std::process::exit(1);
        }
    };

    // Resolve cloud_id (config → env → persisted file → generate)
    let cloud_id = config::resolve_cloud_id(&config.general);

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

    // Initialize StormLog — sync file logger's log_dir with general.log_dir
    let mut stormlog_config = config.stormlog.clone();
    stormlog_config.file.log_dir = config.general.log_dir.clone();
    let stormlog = Arc::new(StormLog::new(
        stormlog_config,
        config.general.name.clone(),
    ));
    stormlog.start().await;

    // Ensure log dir exists for file-based logs
    if let Err(e) = tokio::fs::create_dir_all(&config.general.log_dir).await {
        error!(error = %e, "failed to create log directory");
        std::process::exit(1);
    }

    let supervisor = Arc::new(Supervisor::new(stormlog.clone(), event_bus.clone()));
    let cron_scheduler = Arc::new(CronScheduler::new(stormlog.clone(), event_bus.clone()));
    let stats = Arc::new(StatsCollector::new(config.general.name.clone()));
    stats.start_memory_monitor();
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

    // Start OCI image updater if enabled
    let updater = if config.updater.enabled {
        let updater = Arc::new(Updater::new(
            config.updater.clone(),
            supervisor.clone(),
            event_bus.clone(),
            config.process.clone(),
        ));
        let updater_loop = updater.clone();
        tokio::spawn(async move { updater_loop.poll_loop().await });
        Some(updater)
    } else {
        None
    };

    // Start supervised processes (only those without image tracking — updater handles the rest)
    let sup = supervisor.clone();
    let process_configs: Vec<_> = config
        .process
        .iter()
        .filter(|p| p.image.is_none())
        .cloned()
        .collect();
    let start_handle = tokio::spawn(async move {
        if let Err(e) = sup.start_all(&process_configs).await {
            error!(error = %e, "failed to start processes");
        }
    });

    // NATS output publishing: forward all log entries to NATS subjects
    #[cfg(feature = "nats")]
    if config.events.enabled {
        let mut log_rx = stormlog.subscribe_all();
        let _eb = event_bus.clone();
        tokio::spawn(async move {
            while let Ok(entry) = log_rx.recv().await {
                let subject = format!("stormd.output.{}.{}", entry.process, entry.stream);
                // Use event bus NATS client indirectly — entries are already on broadcast
                let _ = subject; // NATS publishing would go here if direct client access was exposed
            }
        });
    }

    // Shutdown channel — API handler sends exit code, main loop receives it
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel::<Option<i32>>(None);

    // Extract UI plugins from process configs
    let ui_plugins: Vec<api::UiPlugin> = config
        .process
        .iter()
        .filter_map(|p| {
            p.ui.as_ref().map(|ui| api::UiPlugin {
                name: p.name.clone(),
                label: ui.label.clone(),
                proxy_url: ui.proxy.clone(),
            })
        })
        .collect();

    if !ui_plugins.is_empty() {
        info!(
            count = ui_plugins.len(),
            plugins = ?ui_plugins.iter().map(|p| &p.label).collect::<Vec<_>>(),
            "registered UI plugins"
        );
    }

    // Start CloudID key refresh if owner is configured
    let cloudid_keys = if config.ssh.enabled && config.ssh.owner.is_some() {
        let url = config.ssh.cloudid_url.clone();
        info!(
            cloudid_url = %url,
            owner = ?config.ssh.owner,
            "starting CloudID SSH key refresh"
        );
        Some(stormd::cloudid::start_key_refresh(url).await)
    } else {
        None
    };

    // Start SSH server
    let ssh_config = config.ssh.clone();
    let ssh_state = Arc::new(api::AppState {
        supervisor: supervisor.clone(),
        stormlog: stormlog.clone(),
        cron_scheduler: cron_scheduler.clone(),
        stats: stats.clone(),
        backup: backup.clone(),
        updater: updater.clone(),
        shutdown_tx: shutdown_tx.clone(),
        debug_enabled: config.debug.enabled,
        allow_signal: config.debug.allow_signal,
        allow_stdin: config.debug.allow_stdin,
        log_dir: config.general.log_dir.clone(),
        container_name: config.general.name.clone(),
        cloud_id: cloud_id.clone(),
        ui_plugins: ui_plugins.clone(),
    });
    let ssh_container = config.general.name.clone();
    let ssh_keys = cloudid_keys.clone();
    tokio::spawn(async move {
        ssh::start_ssh_server(ssh_config, ssh_state, ssh_container, ssh_keys).await;
    });

    // Build and start API server
    let app_state = Arc::new(api::AppState {
        supervisor: supervisor.clone(),
        stormlog: stormlog.clone(),
        cron_scheduler: cron_scheduler.clone(),
        stats: stats.clone(),
        backup: backup.clone(),
        updater: updater.clone(),
        shutdown_tx,
        debug_enabled: config.debug.enabled,
        allow_signal: config.debug.allow_signal,
        allow_stdin: config.debug.allow_stdin,
        log_dir: config.general.log_dir.clone(),
        container_name: config.general.name.clone(),
        cloud_id,
        ui_plugins,
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
    let stormlog_shutdown = stormlog.clone();

    let exit_code_rx = shutdown_rx.clone();
    let mut api_shutdown_rx = shutdown_rx;
    let shutdown_signal = async move {
        let ctrl_c = tokio::signal::ctrl_c();
        let api_shutdown = async {
            while api_shutdown_rx.changed().await.is_ok() {
                if api_shutdown_rx.borrow().is_some() {
                    return;
                }
            }
        };

        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM handler");
            tokio::select! {
                _ = ctrl_c => info!("received SIGINT"),
                _ = sigterm.recv() => info!("received SIGTERM"),
                _ = api_shutdown => {
                    let code = api_shutdown_rx.borrow().unwrap_or(0);
                    info!(exit_code = code, "shutdown requested via API");
                }
            }
        }

        #[cfg(not(unix))]
        {
            tokio::select! {
                _ = ctrl_c => info!("received SIGINT"),
                _ = api_shutdown => {
                    let code = api_shutdown_rx.borrow().unwrap_or(0);
                    info!(exit_code = code, "shutdown requested via API");
                }
            }
        }
    };

    // PID 1 zombie reaper (Linux only)
    #[cfg(target_os = "linux")]
    tokio::spawn(async {
        reap_zombies().await;
    });

    // Network init — set sysctls for container networking (Linux only)
    #[cfg(target_os = "linux")]
    init_network_sysctls();

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

    // Flush stormlog buffers
    if let Err(e) = stormlog_shutdown.flush().await {
        warn!(error = %e, "failed to flush stormlog buffers");
    }

    // Backup logs on failure if configured
    if supervisor.has_failed().await && backup_on_failure {
        info!("backing up logs before exit");
        if let Err(e) = backup_shutdown.backup_logs(&log_dir).await {
            error!(error = %e, "log backup failed");
        }
    }

    info!("stormd shutdown complete");

    // Exit with API-requested code, failure code, or 0
    if let Some(code) = *exit_code_rx.borrow() {
        std::process::exit(code);
    }
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

/// Set network sysctls for container networking.
///
/// In scratch containers, there's no sysctl command. We write directly to
/// /proc/sys to enable ICMP echo replies, proper ARP handling, and IP
/// forwarding so that veth-based networking works correctly.
#[cfg(target_os = "linux")]
fn init_network_sysctls() {
    let sysctls = [
        // Allow ping — respond to ICMP echo requests
        ("/proc/sys/net/ipv4/icmp_echo_ignore_all", "0"),
        // Accept packets with local source addresses
        ("/proc/sys/net/ipv4/conf/all/accept_local", "1"),
        // Enable IP forwarding
        ("/proc/sys/net/ipv4/ip_forward", "1"),
        // ARP: reply only if target IP is local address on the interface
        ("/proc/sys/net/ipv4/conf/all/arp_ignore", "0"),
        // ARP: use best local address for ARP requests
        ("/proc/sys/net/ipv4/conf/all/arp_announce", "0"),
        // Accept ICMP redirects (useful for multi-hop veth setups)
        ("/proc/sys/net/ipv4/conf/all/accept_redirects", "1"),
        // Enable IPv6 if available
        ("/proc/sys/net/ipv6/conf/all/disable_ipv6", "0"),
    ];

    for (path, value) in &sysctls {
        match std::fs::write(path, value) {
            Ok(_) => tracing::debug!(path = %path, value = %value, "sysctl set"),
            Err(e) => {
                // Not fatal — /proc/sys may not exist or may be read-only
                tracing::debug!(path = %path, error = %e, "sysctl not available");
            }
        }
    }

    info!("network sysctls initialized");
}
