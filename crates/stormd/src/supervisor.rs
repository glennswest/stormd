use crate::config::{FailureAction, LivenessProbe, ProbeType, ProcessConfig};
use crate::events::{EventBus, EventKind};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use stormlog::StormLog;
use tokio::process::Command;
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{error, info, warn};

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ProcessState {
    Pending,
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
    Restarting,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessStatus {
    pub name: String,
    pub state: ProcessState,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub started_at: Option<DateTime<Utc>>,
    pub stopped_at: Option<DateTime<Utc>>,
    pub restarts: u32,
    pub crashes: u32,
    pub restart_timestamps: Vec<DateTime<Utc>>,
    pub uptime_secs: Option<i64>,
    pub liveness_failures: u32,
    pub has_liveness: bool,
    pub liveness_config: Option<crate::config::LivenessProbe>,
}

struct ManagedProcess {
    config: ProcessConfig,
    state: ProcessState,
    pid: Option<u32>,
    exit_code: Option<i32>,
    started_at: Option<DateTime<Utc>>,
    stopped_at: Option<DateTime<Utc>>,
    restarts: u32,
    crashes: u32,
    restart_timestamps: Vec<DateTime<Utc>>,
    kill_tx: Option<tokio::sync::oneshot::Sender<()>>,
    stdin_tx: Option<tokio::sync::mpsc::Sender<String>>,
    liveness_failures: u32,
}

impl ManagedProcess {
    fn status(&self) -> ProcessStatus {
        let uptime = self.started_at.map(|s| {
            if self.state == ProcessState::Running {
                (Utc::now() - s).num_seconds()
            } else {
                self.stopped_at
                    .map(|e| (e - s).num_seconds())
                    .unwrap_or(0)
            }
        });
        ProcessStatus {
            name: self.config.name.clone(),
            state: self.state.clone(),
            pid: self.pid,
            exit_code: self.exit_code,
            started_at: self.started_at,
            stopped_at: self.stopped_at,
            restarts: self.restarts,
            crashes: self.crashes,
            restart_timestamps: self.restart_timestamps.clone(),
            uptime_secs: uptime,
            liveness_failures: self.liveness_failures,
            has_liveness: self.config.liveness.is_some(),
            liveness_config: self.config.liveness.clone(),
        }
    }

    fn restart_count_in_window(&self, window_secs: u64) -> u32 {
        let cutoff = Utc::now() - chrono::Duration::seconds(window_secs as i64);
        self.restart_timestamps
            .iter()
            .filter(|t| **t > cutoff)
            .count() as u32
    }
}

struct ExitEvent {
    name: String,
    exit_code: Option<i32>,
}

pub struct Supervisor {
    processes: RwLock<HashMap<String, Arc<Mutex<ManagedProcess>>>>,
    stormlog: Arc<StormLog>,
    event_bus: Arc<EventBus>,
    container_failed: RwLock<bool>,
    exit_tx: mpsc::Sender<ExitEvent>,
    exit_rx: Mutex<Option<mpsc::Receiver<ExitEvent>>>,
}

impl Supervisor {
    pub fn new(stormlog: Arc<StormLog>, event_bus: Arc<EventBus>) -> Self {
        let (exit_tx, exit_rx) = mpsc::channel(64);
        Self {
            processes: RwLock::new(HashMap::new()),
            stormlog,
            event_bus,
            container_failed: RwLock::new(false),
            exit_tx,
            exit_rx: Mutex::new(Some(exit_rx)),
        }
    }

    pub async fn has_failed(&self) -> bool {
        *self.container_failed.read().await
    }

    pub async fn run_exit_handler(self: &Arc<Self>) {
        let mut rx = self.exit_rx.lock().await.take().expect("exit handler already running");
        while let Some(evt) = rx.recv().await {
            self.handle_exit(&evt.name, evt.exit_code).await;
        }
    }

    pub async fn start_all(self: &Arc<Self>, configs: &[ProcessConfig]) -> anyhow::Result<()> {
        for cfg in configs {
            let proc = Arc::new(Mutex::new(ManagedProcess {
                config: cfg.clone(),
                state: ProcessState::Pending,
                pid: None,
                exit_code: None,
                started_at: None,
                stopped_at: None,
                restarts: 0,
                crashes: 0,
                restart_timestamps: Vec::new(),
                kill_tx: None,
                stdin_tx: None,
                liveness_failures: 0,
            }));
            self.processes.write().await.insert(cfg.name.clone(), proc);
        }

        for cfg in configs {
            self.wait_for_dependencies(&cfg.depends_on).await;

            if cfg.startup_delay_secs > 0 {
                tokio::time::sleep(tokio::time::Duration::from_secs(cfg.startup_delay_secs)).await;
            }

            self.spawn_process(&cfg.name).await?;
        }

        Ok(())
    }

    async fn wait_for_dependencies(&self, deps: &[String]) {
        for dep in deps {
            loop {
                let procs = self.processes.read().await;
                if let Some(p) = procs.get(dep) {
                    let p = p.lock().await;
                    if p.state == ProcessState::Running {
                        break;
                    }
                }
                drop(procs);
                tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
            }
        }
    }

    async fn spawn_process(self: &Arc<Self>, name: &str) -> anyhow::Result<()> {
        let procs = self.processes.read().await;
        let proc_arc = procs
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("process not found: {}", name))?;
        drop(procs);

        let config = {
            let mut proc = proc_arc.lock().await;
            proc.state = ProcessState::Starting;
            proc.config.clone()
        };

        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        for (k, v) in &config.env {
            cmd.env(k, v);
        }
        if let Some(dir) = &config.working_dir {
            cmd.current_dir(dir);
        }

        let mut child = cmd.spawn()?;
        let pid = child.id();

        // Set up stdin channel
        let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<String>(64);
        if let Some(mut child_stdin) = child.stdin.take() {
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                while let Some(line) = stdin_rx.recv().await {
                    if child_stdin.write_all(line.as_bytes()).await.is_err() {
                        break;
                    }
                    if child_stdin.write_all(b"\n").await.is_err() {
                        break;
                    }
                }
            });
        }

        // Capture stdout/stderr via stormlog
        self.stormlog.spawn_capture(
            config.name.clone(),
            child.stdout.take(),
            child.stderr.take(),
        ).await;

        // Update state
        {
            let mut proc = proc_arc.lock().await;
            proc.state = ProcessState::Running;
            proc.pid = pid;
            proc.exit_code = None;
            proc.started_at = Some(Utc::now());
            proc.stopped_at = None;
            proc.stdin_tx = Some(stdin_tx);
        }

        info!(process = %config.name, pid = ?pid, "process started");
        self.event_bus
            .emit_simple(EventKind::ProcessStarted, Some(config.name.clone()))
            .await;

        // Monitor task
        let (kill_tx, mut kill_rx) = tokio::sync::oneshot::channel::<()>();
        {
            let mut proc = proc_arc.lock().await;
            proc.kill_tx = Some(kill_tx);
        }

        let exit_tx = self.exit_tx.clone();
        let name_owned = config.name.clone();
        let proc_arc_clone = proc_arc.clone();
        tokio::spawn(async move {
            tokio::select! {
                status = child.wait() => {
                    let exit_code = status.ok().and_then(|s| s.code());
                    let _ = exit_tx.send(ExitEvent { name: name_owned, exit_code }).await;
                }
                _ = &mut kill_rx => {
                    let _ = child.kill().await;
                    let mut proc = proc_arc_clone.lock().await;
                    proc.state = ProcessState::Stopped;
                    proc.stopped_at = Some(Utc::now());
                    proc.pid = None;
                    info!(process = %name_owned, "process killed by request");
                }
            }
        });

        // Spawn liveness monitor if configured
        if let Some(liveness) = &config.liveness {
            let supervisor = Arc::clone(self);
            let name = config.name.clone();
            let liveness = liveness.clone();
            let proc_arc_liveness = proc_arc.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(liveness.initial_delay_secs)).await;
                loop {
                    tokio::time::sleep(Duration::from_secs(liveness.interval_secs)).await;

                    // Check if process is still running
                    let state = {
                        let proc = proc_arc_liveness.lock().await;
                        proc.state.clone()
                    };
                    if state != ProcessState::Running {
                        break;
                    }

                    let ok = execute_probe(&liveness).await;
                    if ok {
                        // Reset failure counter
                        let mut proc = proc_arc_liveness.lock().await;
                        proc.liveness_failures = 0;
                    } else {
                        let failures = {
                            let mut proc = proc_arc_liveness.lock().await;
                            proc.liveness_failures += 1;
                            proc.liveness_failures
                        };
                        warn!(process=%name, failures, "liveness check failed");

                        if failures >= liveness.failure_threshold {
                            error!(process=%name, "liveness threshold exceeded — sending SIGUSR1");
                            let _ = supervisor.signal_process(&name, "SIGUSR1").await;
                            supervisor.event_bus.emit_simple(
                                EventKind::LivenessCheckFailed,
                                Some(name.clone()),
                            ).await;

                            // Wait 5 seconds for graceful death
                            tokio::time::sleep(Duration::from_secs(5)).await;

                            // Check if still running
                            let still_running = {
                                let proc = proc_arc_liveness.lock().await;
                                proc.state == ProcessState::Running
                            };
                            if still_running {
                                error!(process=%name, "still running after SIGUSR1 — SIGKILL");
                                let _ = supervisor.signal_process(&name, "SIGKILL").await;
                            }
                            break;
                        }
                    }
                }
            });
        }

        Ok(())
    }

    async fn handle_exit(self: &Arc<Self>, name: &str, exit_code: Option<i32>) {
        let procs = self.processes.read().await;
        let proc_arc = match procs.get(name) {
            Some(p) => p.clone(),
            None => return,
        };
        drop(procs);

        let (failure_action, exit_action, restart_delay, max_restarts, window, restarts_in_window) = {
            let mut proc = proc_arc.lock().await;
            proc.exit_code = exit_code;
            proc.stopped_at = Some(Utc::now());
            proc.pid = None;

            let in_window = proc.restart_count_in_window(proc.config.restart_window_secs);
            (
                proc.config.on_failure.clone(),
                proc.config.on_exit.clone(),
                proc.config.restart_delay_secs,
                proc.config.max_restarts,
                proc.config.restart_window_secs,
                in_window,
            )
        };

        let success = exit_code == Some(0);
        let failed = !success;

        // Increment crash counter and emit crash entry at Emergency severity BEFORE archiving
        if failed {
            {
                let mut proc = proc_arc.lock().await;
                proc.crashes += 1;
            }
            self.stormlog.emit_crash(name, exit_code).await;
        }

        // Archive this run's logs to MinIO and free local disk space
        self.stormlog.archive_run(name, failed).await;

        if success {
            match exit_action {
                crate::config::ExitAction::Restart => {
                    info!(process = %name, "process exited cleanly — restarting (on_exit=restart)");
                    self.event_bus
                        .emit_simple(EventKind::ProcessStopped, Some(name.to_string()))
                        .await;
                    // Fall through to restart logic below
                }
                crate::config::ExitAction::Stop => {
                    let mut proc = proc_arc.lock().await;
                    proc.state = ProcessState::Stopped;
                    info!(process = %name, "process exited cleanly — stopping (on_exit=stop)");
                    self.event_bus
                        .emit_simple(EventKind::ProcessStopped, Some(name.to_string()))
                        .await;
                    return;
                }
            }
        } else {
            warn!(process = %name, code = ?exit_code, "process exited with error");
            self.event_bus
                .emit_simple(EventKind::ProcessCrashed, Some(name.to_string()))
                .await;
        }

        // For clean exits with on_exit=restart, we use the restart logic
        // but skip the on_failure check (it already exited cleanly).
        if success {
            if restarts_in_window >= max_restarts {
                let mut proc = proc_arc.lock().await;
                proc.state = ProcessState::Stopped;
                warn!(
                    process = %name,
                    restarts = restarts_in_window,
                    max = max_restarts,
                    "max restarts exceeded for clean exit — stopping"
                );
                return;
            }
            {
                let mut proc = proc_arc.lock().await;
                proc.state = ProcessState::Restarting;
                proc.restarts += 1;
                proc.restart_timestamps.push(Utc::now());
            }
            self.event_bus
                .emit_simple(EventKind::ProcessRestarting, Some(name.to_string()))
                .await;
            tokio::time::sleep(tokio::time::Duration::from_secs(restart_delay)).await;
            if let Err(e) = self.spawn_process(name).await {
                error!(process = %name, error = %e, "failed to restart process after clean exit");
                let mut proc = proc_arc.lock().await;
                proc.state = ProcessState::Failed;
            }
            return;
        }

        match failure_action {
            FailureAction::Fail => {
                let mut proc = proc_arc.lock().await;
                proc.state = ProcessState::Failed;
                error!(process = %name, "failure action is 'fail' — failing container");
                *self.container_failed.write().await = true;
                self.event_bus
                    .emit_simple(EventKind::ContainerFailing, None)
                    .await;
            }
            FailureAction::Restart => {
                if restarts_in_window >= max_restarts {
                    let mut proc = proc_arc.lock().await;
                    proc.state = ProcessState::Failed;
                    error!(
                        process = %name,
                        restarts = restarts_in_window,
                        max = max_restarts,
                        window_secs = window,
                        "max restarts exceeded — failing container"
                    );
                    *self.container_failed.write().await = true;
                    self.event_bus
                        .emit_simple(EventKind::ContainerFailing, None)
                        .await;
                } else {
                    {
                        let mut proc = proc_arc.lock().await;
                        proc.state = ProcessState::Restarting;
                        proc.restarts += 1;
                        proc.restart_timestamps.push(Utc::now());
                    }
                    info!(
                        process = %name,
                        delay_secs = restart_delay,
                        "restarting process"
                    );
                    self.event_bus
                        .emit_simple(EventKind::ProcessRestarting, Some(name.to_string()))
                        .await;
                    tokio::time::sleep(tokio::time::Duration::from_secs(restart_delay)).await;
                    if let Err(e) = self.spawn_process(name).await {
                        error!(process = %name, error = %e, "failed to restart process");
                        let mut proc = proc_arc.lock().await;
                        proc.state = ProcessState::Failed;
                        *self.container_failed.write().await = true;
                    }
                }
            }
            FailureAction::Ignore => {
                let mut proc = proc_arc.lock().await;
                proc.state = ProcessState::Stopped;
                info!(process = %name, "failure action is 'ignore' — leaving stopped");
            }
        }
    }

    pub async fn stop_process(&self, name: &str) -> anyhow::Result<()> {
        let procs = self.processes.read().await;
        let proc_arc = procs
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("process not found: {}", name))?;
        drop(procs);

        let mut proc = proc_arc.lock().await;
        if proc.state != ProcessState::Running {
            anyhow::bail!("process '{}' is not running (state: {:?})", name, proc.state);
        }
        proc.state = ProcessState::Stopping;
        if let Some(tx) = proc.kill_tx.take() {
            let _ = tx.send(());
        }
        self.event_bus
            .emit_simple(EventKind::ProcessStopped, Some(name.to_string()))
            .await;
        Ok(())
    }

    pub async fn restart_process(self: &Arc<Self>, name: &str) -> anyhow::Result<()> {
        {
            let procs = self.processes.read().await;
            if let Some(proc_arc) = procs.get(name) {
                let proc = proc_arc.lock().await;
                if proc.state == ProcessState::Running {
                    drop(proc);
                    drop(procs);
                    self.stop_process(name).await?;
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                }
            }
        }
        self.spawn_process(name).await
    }

    pub async fn start_process(self: &Arc<Self>, name: &str) -> anyhow::Result<()> {
        {
            let procs = self.processes.read().await;
            let proc_arc = procs
                .get(name)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("process not found: {}", name))?;
            let proc = proc_arc.lock().await;
            if proc.state == ProcessState::Running {
                anyhow::bail!("process '{}' is already running", name);
            }
        }
        self.spawn_process(name).await
    }

    pub async fn send_stdin(&self, name: &str, input: &str) -> anyhow::Result<()> {
        let procs = self.processes.read().await;
        let proc_arc = procs
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("process not found: {}", name))?;
        let proc = proc_arc.lock().await;
        if let Some(tx) = &proc.stdin_tx {
            tx.send(input.to_string())
                .await
                .map_err(|_| anyhow::anyhow!("stdin channel closed"))?;
            Ok(())
        } else {
            anyhow::bail!("no stdin channel for process '{}'", name)
        }
    }

    pub async fn get_status(&self, name: &str) -> anyhow::Result<ProcessStatus> {
        let procs = self.processes.read().await;
        let proc_arc = procs
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("process not found: {}", name))?;
        let proc = proc_arc.lock().await;
        Ok(proc.status())
    }

    pub async fn get_all_statuses(&self) -> Vec<ProcessStatus> {
        let procs = self.processes.read().await;
        let mut statuses = Vec::new();
        for p in procs.values() {
            let proc = p.lock().await;
            statuses.push(proc.status());
        }
        statuses.sort_by(|a, b| a.name.cmp(&b.name));
        statuses
    }

    pub async fn stop_all(&self) {
        let procs = self.processes.read().await;
        for (name, p) in procs.iter() {
            let mut proc = p.lock().await;
            if proc.state == ProcessState::Running {
                proc.state = ProcessState::Stopping;
                if let Some(tx) = proc.kill_tx.take() {
                    let _ = tx.send(());
                    info!(process = %name, "stopping process");
                }
            }
        }
    }

    /// Update a process's runtime config (command, args, env, working_dir) without
    /// removing it from the process map. Used by the updater to change the command
    /// derived from an OCI image config before restarting.
    pub async fn update_process_config(&self, name: &str, config: ProcessConfig) -> anyhow::Result<()> {
        let procs = self.processes.read().await;
        let proc_arc = procs
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("process not found: {}", name))?;
        drop(procs);

        let mut proc = proc_arc.lock().await;
        proc.config = config;
        info!(process = %name, command = %proc.config.command, "process config updated");
        Ok(())
    }

    /// Register a new process without starting it.
    pub async fn register_process(&self, config: ProcessConfig) {
        let proc = Arc::new(Mutex::new(ManagedProcess {
            config: config.clone(),
            state: ProcessState::Pending,
            pid: None,
            exit_code: None,
            started_at: None,
            stopped_at: None,
            restarts: 0,
            crashes: 0,
            restart_timestamps: Vec::new(),
            kill_tx: None,
            stdin_tx: None,
            liveness_failures: 0,
        }));
        self.processes.write().await.insert(config.name.clone(), proc);
    }

    /// Get names of all registered processes.
    pub async fn process_names(&self) -> Vec<String> {
        let procs = self.processes.read().await;
        procs.keys().cloned().collect()
    }

    /// Send a signal to a running process by name.
    pub async fn signal_process(&self, name: &str, signal: &str) -> anyhow::Result<()> {
        let procs = self.processes.read().await;
        let proc_arc = procs
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("process not found: {}", name))?;
        drop(procs);

        let proc = proc_arc.lock().await;
        let pid = proc.pid.ok_or_else(|| anyhow::anyhow!("process has no pid"))?;

        #[cfg(target_os = "linux")]
        {
            use nix::sys::signal::Signal;
            let sig = match signal {
                "SIGUSR1" | "USR1" => Signal::SIGUSR1,
                "SIGKILL" | "KILL" => Signal::SIGKILL,
                "SIGTERM" | "TERM" => Signal::SIGTERM,
                _ => anyhow::bail!("unsupported signal: {}", signal),
            };
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), sig)?;
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = (pid, signal);
        }

        Ok(())
    }
}

async fn execute_probe(probe: &LivenessProbe) -> bool {
    let timeout = Duration::from_secs(probe.timeout_secs);
    match &probe.probe {
        ProbeType::Http { url } => {
            match tokio::time::timeout(timeout, reqwest::get(url)).await {
                Ok(Ok(resp)) => resp.status().is_success() || resp.status().is_redirection(),
                _ => false,
            }
        }
        ProbeType::Tcp { port } => {
            let addr = format!("127.0.0.1:{}", port);
            matches!(
                tokio::time::timeout(timeout, tokio::net::TcpStream::connect(&addr)).await,
                Ok(Ok(_))
            )
        }
    }
}
