use crate::config::{FailureAction, LivenessProbe, NoRestartAction, ProbeType, ProcessConfig};
use crate::events::{EventBus, EventKind};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
    /// Which run this is (bumped at every spawn).
    pub run: u64,
    /// Its ready_probe has passed for this run (true when it has none).
    pub ready: bool,
    pub ready_at: Option<DateTime<Utc>>,
    /// When the liveness probe last passed, and for which run (stormd#44).
    pub liveness_passed_at: Option<DateTime<Utc>>,
    pub liveness_passed_run: Option<u64>,
    /// Every check it has passed for this run (see `Supervisor::health_of`).
    pub healthy: bool,
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
    /// Which run this is: bumped at every spawn. A liveness task belongs to
    /// one run and acts only while it is still the current one (stormd#45).
    run: u64,
    /// Liveness tasks alive for this process — one at most, by construction.
    liveness_tasks: Arc<AtomicUsize>,
    /// Has this process's `ready_probe` passed since it last started?
    ///
    /// Separate from `Running`, because they answer different questions. A
    /// datastore is *running* the moment it is forked and *ready* when it will
    /// answer — and a dependent started between those two points fails against
    /// something that is plainly there, which is the confusing kind.
    ///
    /// `true` when there is no probe: a process with nothing to check is ready
    /// when it is running, which is the previous behaviour.
    ready: bool,
    ready_at: Option<DateTime<Utc>>,
    liveness_passed_at: Option<DateTime<Utc>>,
    liveness_passed_run: Option<u64>,
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
            run: self.run,
            ready: self.ready,
            ready_at: self.ready_at,
            liveness_passed_at: self.liveness_passed_at,
            liveness_passed_run: self.liveness_passed_run,
            healthy: false,
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
    /// Set by `stop_all`. From then on nothing new is spawned — not the rest
    /// of the start order, not a restart, not an API start — and a dependency
    /// wait gives up. See `stop_all`.
    shutting_down: AtomicBool,
    /// The health of every process's declared APIs (stormd#49).
    api_health: Arc<crate::apihealth::ApiHealthStore>,
    /// Presents the goldens processes name (stormd#36); set by main when any does.
    goldens: std::sync::OnceLock<Arc<crate::goldens::Goldens>>,
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
            shutting_down: AtomicBool::new(false),
            goldens: std::sync::OnceLock::new(),
            api_health: Arc::new(crate::apihealth::ApiHealthStore::default()),
        }
    }

    pub fn api_health(&self) -> &Arc<crate::apihealth::ApiHealthStore> {
        &self.api_health
    }

    /// Probe one of a run's APIs until the run ends (the task is aborted with
    /// it). With `restart_after_stalled_secs`, a stall that long ends the run:
    /// SIGTERM, SIGKILL after its stop timeout, and the restart policy takes
    /// the exit.
    async fn watch_api(
        &self,
        name: &str,
        proc_arc: &Arc<Mutex<ManagedProcess>>,
        run: u64,
        pid: Option<u32>,
        api: &crate::config::ApiProbe,
    ) {
        use crate::apihealth::ApiState;
        let client = match crate::apihealth::client(api) {
            Ok(c) => c,
            Err(e) => {
                error!(process = %name, api = %api.name, error = %e, "API probe cannot be set up — not probing it");
                return;
            }
        };
        tokio::time::sleep(Duration::from_secs(api.initial_delay_secs)).await;
        loop {
            {
                let p = proc_arc.lock().await;
                if p.run != run || p.state != ProcessState::Running {
                    return;
                }
            }
            let seen = crate::apihealth::probe(&client, api).await;
            let (state, since) = self.api_health.record(name, api, seen, Utc::now());
            if let (ApiState::Stalled, Some(limit)) = (state, api.restart_after_stalled_secs) {
                let stalled = (Utc::now() - since).num_seconds().max(0) as u64;
                if stalled >= limit {
                    error!(
                        process = %name, api = %api.name, stalled_secs = stalled,
                        "API stalled past restart_after_stalled_secs — restarting the process"
                    );
                    let stop = Duration::from_secs(proc_arc.lock().await.config.stop_timeout_secs);
                    if self.signal_run(proc_arc, run, pid, "SIGTERM").await {
                        tokio::time::sleep(stop).await;
                        let still = {
                            let p = proc_arc.lock().await;
                            p.run == run && p.state == ProcessState::Running
                        };
                        if still {
                            let _ = self.signal_run(proc_arc, run, pid, "SIGKILL").await;
                        }
                    }
                    return;
                }
            }
            tokio::time::sleep(Duration::from_secs(api.interval_secs)).await;
        }
    }

    pub fn set_goldens(&self, goldens: Arc<crate::goldens::Goldens>) {
        let _ = self.goldens.set(goldens);
    }

    pub fn goldens(&self) -> Option<&Arc<crate::goldens::Goldens>> {
        self.goldens.get()
    }

    /// Present a process's goldens before it starts; false when shutdown
    /// began first.
    async fn present_goldens(&self, cfg: &ProcessConfig) -> bool {
        match (&self.goldens.get(), cfg.golden.is_empty()) {
            (_, true) => true,
            (None, false) => {
                warn!(process = %cfg.name, "names goldens but no golden presenter is set — starting without them");
                true
            }
            (Some(g), false) => g.present_all(&cfg.name, &cfg.golden, || self.is_shutting_down()).await,
        }
    }

    /// Swap one of a process's goldens for another (a new release): stop the
    /// process, release the old golden, present the new one, start the
    /// process again. In memory only — a stormd restart goes back to the
    /// config. If the new one cannot be presented, the old one is put back.
    pub async fn swap_golden(
        self: &Arc<Self>,
        process: &str,
        name: &str,
        golden: Option<String>,
        volume_id: Option<String>,
    ) -> anyhow::Result<crate::goldens::Presented> {
        let goldens = self.goldens.get().cloned().ok_or_else(|| anyhow::anyhow!("no goldens configured"))?;
        if golden.is_some() == volume_id.is_some() {
            anyhow::bail!("give exactly one of golden and volume_id");
        }
        let proc_arc = self
            .processes
            .read()
            .await
            .get(process)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("process not found: {}", process))?;
        let old = {
            let p = proc_arc.lock().await;
            p.config
                .golden
                .iter()
                .find(|g| g.name == name)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("process '{process}' names no golden '{name}'"))?
        };
        let new = crate::config::GoldenMount { golden, volume_id, ..old.clone() };

        let was_running = proc_arc.lock().await.state == ProcessState::Running;
        if was_running {
            self.stop_process(process).await?;
            self.wait_stopped(process).await;
        }
        if let Err(e) = goldens.release(process, name).await {
            if was_running {
                self.spawn_process(process).await?;
            }
            return Err(e);
        }
        let result = match goldens.present(process, &new).await {
            Ok(p) => {
                let mut pr = proc_arc.lock().await;
                if let Some(g) = pr.config.golden.iter_mut().find(|g| g.name == name) {
                    *g = new;
                }
                Ok(p)
            }
            Err(e) => {
                warn!(process, golden = %name, error = %e, "new golden not presented — putting the old one back");
                if let Err(e2) = goldens.present(process, &old).await {
                    warn!(process, golden = %name, error = %e2, "old golden not presented either");
                }
                Err(e)
            }
        };
        if was_running {
            self.spawn_process(process).await?;
        }
        result
    }

    fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    pub async fn has_failed(&self) -> bool {
        *self.container_failed.read().await
    }

    /// Take exit events, each on its own task.
    ///
    /// **One process's cooloff held up every other exit** (stormd#22). This
    /// awaited `handle_exit` in turn, and `handle_exit` sleeps the restart
    /// cooloff before respawning — so while one crash-looping process waited
    /// out its (up to 30 s) cooloff, every other process that died stayed
    /// `running` in the API, unrestarted and invisible to dependency checks.
    /// A wave of processes in the #15 long suite took ~100 ms each to settle
    /// for that reason alone. Exits of *one* process cannot overlap: it exits
    /// again only after `handle_exit` has respawned it.
    pub async fn run_exit_handler(self: &Arc<Self>) {
        let mut rx = self.exit_rx.lock().await.take().expect("exit handler already running");
        while let Some(evt) = rx.recv().await {
            let this = Arc::clone(self);
            tokio::spawn(async move { this.handle_exit(&evt.name, evt.exit_code).await });
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
                run: 0,
                liveness_tasks: Arc::new(AtomicUsize::new(0)),
                ready: cfg.ready_probe.is_none(),
                ready_at: None,
                liveness_passed_at: None,
                liveness_passed_run: None,
            }));
            self.processes.write().await.insert(cfg.name.clone(), proc);
        }

        for cfg in configs {
            self.wait_for_dependencies(&cfg.depends_on).await;
            self.wait_for_files(&cfg.name, &cfg.wait_for_files).await;
            self.wait_for_node_vars(&cfg.name).await;
            if !self.present_goldens(cfg).await {
                info!(process = %cfg.name, "shutting down — not starting");
                return Ok(());
            }

            if cfg.startup_delay_secs > 0 {
                self.sleep_unless_shutdown(Duration::from_secs(cfg.startup_delay_secs)).await;
            }

            // The start order ends where shutdown begins.
            if self.is_shutting_down() {
                info!(process = %cfg.name, "shutting down — not starting");
                return Ok(());
            }

            self.spawn_process(&cfg.name).await?;
        }

        Ok(())
    }

    /// Drive a process's `ready_probe` until it passes, then mark it ready.
    ///
    /// **`ready_probe` was parsed and never consumed.** Every one in this
    /// stack was inert, including the one meant to stop the kubelet racing the
    /// apiserver — so the ordering an operator wrote down was not the ordering
    /// that happened, and the failure it prevents (a dependent starting
    /// against something forked but not yet answering) looked like the
    /// dependent being broken.
    ///
    /// In the background rather than inline: only actual dependents should
    /// wait, and a slow probe on one process must not delay every unrelated
    /// process behind it.
    ///
    /// **A restart was never ready again** (stormd#46): this ran only from
    /// the start order, while every spawn resets `ready`. Now each run has its
    /// own watch, started by `spawn_process` and ended with the run.
    async fn watch_ready(&self, name: &str, proc_arc: &Arc<Mutex<ManagedProcess>>, run: u64, probe: &crate::config::ReadyProbe) {
        let interval = match probe {
            crate::config::ReadyProbe::Http { interval_secs, .. }
            | crate::config::ReadyProbe::Tcp { interval_secs, .. }
            | crate::config::ReadyProbe::Exec { interval_secs, .. } => (*interval_secs).max(1),
        };
        loop {
            // Stop waiting on a run that is over: a probe against something
            // that has exited never passes, and looping on it hides the exit.
            {
                let p = proc_arc.lock().await;
                if p.run != run || matches!(p.state, ProcessState::Stopped | ProcessState::Failed) {
                    return;
                }
            }
            if execute_ready_probe(probe).await {
                let mut p = proc_arc.lock().await;
                if p.run == run {
                    p.ready = true;
                    p.ready_at = Some(Utc::now());
                    info!(process = %name, "ready");
                }
                return;
            }
            tokio::time::sleep(Duration::from_secs(interval)).await;
        }
    }

    /// Hold a start until the node has a value for every `${NODE_*}` the
    /// process uses (stormd#3), re-resolving every second. Gives up when
    /// shutdown begins (the spawn is then refused anyway).
    ///
    /// **A node with no address failed three layers down.** With its cables
    /// out at boot, `${NODE_IP}` stayed literal, stormcert-init exited 2
    /// parsing `--ip 10.96.0.1,${NODE_IP},127.0.0.1`, and the container was
    /// failed and retried every 300 s with nothing naming the cause. A
    /// process that cannot run until the node has an address is blocked, not
    /// failed: it waits, says why once, and starts when DHCP comes good.
    async fn wait_for_node_vars(&self, name: &str) {
        let Some(proc_arc) = self.processes.read().await.get(name).cloned() else {
            return;
        };
        let started = tokio::time::Instant::now();
        let mut told = false;
        loop {
            let cfg = proc_arc.lock().await.config.clone();
            match node_vars_missing(&cfg, &crate::nodevars::vars()) {
                None => {
                    if told {
                        info!(process = %name, waited_secs = started.elapsed().as_secs(), "node values present — starting");
                    }
                    return;
                }
                Some(why) => {
                    if self.is_shutting_down() {
                        return;
                    }
                    if !told {
                        error!(process = %name, "{why} — waiting, not starting it");
                        told = true;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// Hold a process's first start until every one of `files` exists
    /// (stormd#38), polled like `depends_on`. Gives up when shutdown begins.
    ///
    /// **fastetcd crash-looped on every boot waiting for its cert.** The
    /// pair is minted by another container a few seconds after fastetcd is
    /// started; until then it exited 1, and the restart cool-off it grew
    /// meant its first good start came ~4 s after the cert existed — a delay
    /// the apiserver, which needs fastetcd, inherited on every install and
    /// reboot. Waiting here is no restart: nothing is counted, no cool-off.
    async fn wait_for_files(&self, name: &str, files: &[String]) {
        if files.is_empty() {
            return;
        }
        let vars = crate::nodevars::vars();
        let files: Vec<String> = files.iter().map(|f| crate::nodevars::expand(f, &vars)).collect();
        let started = tokio::time::Instant::now();
        let mut told = false;
        loop {
            let missing = missing_files(&files);
            if missing.is_empty() {
                if told {
                    info!(process = %name, waited_ms = started.elapsed().as_millis() as u64, "files present — starting");
                }
                return;
            }
            if self.is_shutting_down() {
                return;
            }
            if !told {
                info!(process = %name, missing = ?missing, "waiting for files before starting");
                told = true;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn wait_for_dependencies(&self, deps: &[String]) {
        for dep in deps {
            // Said once per dependency, so a dependent held behind a one-shot
            // that failed is visible without a line every poll.
            let mut told = false;
            loop {
                if self.is_shutting_down() {
                    return;
                }
                let procs = self.processes.read().await;
                if let Some(p) = procs.get(dep) {
                    let p = p.lock().await;
                    if dependency_satisfied(
                        &p.state,
                        p.ready,
                        p.config.ready_probe.is_some(),
                        &p.config.on_exit,
                        p.exit_code,
                    ) {
                        break;
                    }
                    let one_shot = p.config.on_exit == crate::config::ExitAction::Stop;
                    if !told
                        && one_shot
                        && matches!(p.state, ProcessState::Stopped | ProcessState::Failed)
                    {
                        warn!(
                            dependency = %dep,
                            state = ?p.state,
                            code = ?p.exit_code,
                            "waiting on a one-shot that did not finish cleanly — dependents stay held"
                        );
                        told = true;
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

        // Nothing starts once the container is stopping; a child forked now
        // would outlive the `stop_all` that was meant to end it.
        if self.is_shutting_down() {
            anyhow::bail!("stormd is shutting down — not starting '{}'", name);
        }

        // Never with a `${NODE_IP}` left in it (stormd#3): say what is missing
        // here, not three layers down as the process's own parse error.
        {
            let cfg = proc_arc.lock().await.config.clone();
            if let Some(why) = node_vars_missing(&cfg, &crate::nodevars::vars()) {
                anyhow::bail!("{why}");
            }
        }

        let (config, run, liveness_tasks) = {
            let mut proc = proc_arc.lock().await;
            proc.state = ProcessState::Starting;
            // A restarted process is not ready until it says so again.
            proc.ready = proc.config.ready_probe.is_none();
            proc.ready_at = None;
            // A new run, judged afresh: the last run's liveness failures are
            // not this one's (stormd#45).
            proc.run += 1;
            proc.liveness_failures = 0;
            (proc.config.clone(), proc.run, proc.liveness_tasks.clone())
        };

        // Fill in what only this node knows — its address above all. See
        // `nodevars`: an image is built once and runs everywhere, so anything
        // naming *this* node cannot be written into the config, and a control
        // plane that advertises 127.0.0.1 is a cluster of one per machine.
        //
        // At spawn rather than at load, so a node that changes address picks
        // the new one up on the next restart rather than at the next boot.
        let vars = crate::nodevars::vars();
        let args: Vec<String> =
            config.args.iter().map(|a| crate::nodevars::expand(a, &vars)).collect();

        let mut cmd = Command::new(&config.command);
        cmd.args(&args);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        for (k, v) in process_env(
            &config.env,
            &config.env_default,
            |k| std::env::var_os(k).is_some(),
            &vars,
        ) {
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

        // Liveness, one task for this run, ended by the monitor below when
        // the run ends (stormd#45).
        //
        // **A liveness task used to outlive its run.** It stopped only when it
        // read `state != Running` — so one asleep in `initial_delay_secs`, or
        // mid-probe, while its process crashed and was restarted, woke on the
        // *new* run and probed it at once, before that run's own delay. Each
        // such restart left one more, the failure count was never reset, and
        // once a slow start had tripped the threshold every later run was
        // killed seconds after it started: the control plane crash-looped.
        let liveness_task = config.liveness.clone().map(|liveness| {
            let supervisor = Arc::clone(self);
            let name = config.name.clone();
            let proc_arc = proc_arc.clone();
            let guard = TaskCount::enter(liveness_tasks);
            tokio::spawn(async move {
                let _guard = guard;
                supervisor.watch_liveness(&name, &proc_arc, run, pid, &liveness).await;
            })
        });

        // API health probes, one task per API for this run (stormd#49).
        let mut run_tasks: Vec<tokio::task::JoinHandle<()>> = liveness_task.into_iter().collect();
        // Readiness, watched for every run, restarts included (stormd#46).
        if let Some(probe) = config.ready_probe.clone() {
            let supervisor = Arc::clone(self);
            let name = config.name.clone();
            let proc_arc = proc_arc.clone();
            run_tasks.push(tokio::spawn(async move {
                supervisor.watch_ready(&name, &proc_arc, run, &probe).await;
            }));
        }
        for api in config.api.clone() {
            let supervisor = Arc::clone(self);
            let name = config.name.clone();
            let proc_arc = proc_arc.clone();
            run_tasks.push(tokio::spawn(async move {
                supervisor.watch_api(&name, &proc_arc, run, pid, &api).await;
            }));
        }

        let exit_tx = self.exit_tx.clone();
        let name_owned = config.name.clone();
        let stop_timeout = Duration::from_secs(config.stop_timeout_secs);
        let proc_arc_clone = proc_arc.clone();
        tokio::spawn(async move {
            // However the run ends, its liveness and API tasks end with it.
            let _run_tasks = AbortOnDrop(run_tasks);
            tokio::select! {
                status = child.wait() => {
                    let exit_code = status.ok().and_then(|s| s.code());
                    let _ = exit_tx.send(ExitEvent { name: name_owned, exit_code }).await;
                }
                _ = &mut kill_rx => {
                    let exit_code = stop_child(&mut child, pid, stop_timeout, &name_owned).await;
                    let mut proc = proc_arc_clone.lock().await;
                    proc.state = ProcessState::Stopped;
                    proc.stopped_at = Some(Utc::now());
                    proc.pid = None;
                    proc.exit_code = exit_code;
                    info!(process = %name_owned, exit_code = ?exit_code, "process stopped by request");
                }
            }
        });

        Ok(())
    }

    /// Probe one run's liveness until it fails `failure_threshold` times in
    /// a row (SIGUSR1, then SIGKILL after 5 s) or the run is over. Acts on
    /// `run` only: a check, a count or a signal for a run that is no longer
    /// the current one is dropped, and the signal goes to the run's own pid.
    async fn watch_liveness(
        &self,
        name: &str,
        proc_arc: &Arc<Mutex<ManagedProcess>>,
        run: u64,
        pid: Option<u32>,
        liveness: &LivenessProbe,
    ) {
        let current = |p: &ManagedProcess| p.run == run && p.state == ProcessState::Running;
        tokio::time::sleep(Duration::from_secs(liveness.initial_delay_secs)).await;
        loop {
            tokio::time::sleep(Duration::from_secs(liveness.interval_secs)).await;
            if !current(&*proc_arc.lock().await) {
                return;
            }

            let ok = execute_probe(liveness).await;
            let failures = {
                let mut proc = proc_arc.lock().await;
                if !current(&proc) {
                    return;
                }
                if ok {
                    proc.liveness_failures = 0;
                    proc.liveness_passed_at = Some(Utc::now());
                    proc.liveness_passed_run = Some(run);
                    continue;
                }
                proc.liveness_failures += 1;
                proc.liveness_failures
            };
            warn!(process = %name, failures, "liveness check failed");
            if failures < liveness.failure_threshold {
                continue;
            }

            error!(process = %name, "liveness threshold exceeded — sending SIGUSR1");
            if !self.signal_run(proc_arc, run, pid, "SIGUSR1").await {
                return;
            }
            self.event_bus
                .emit_simple(EventKind::LivenessCheckFailed, Some(name.to_string()))
                .await;

            // Wait 5 seconds for graceful death
            tokio::time::sleep(Duration::from_secs(5)).await;
            if current(&*proc_arc.lock().await) {
                error!(process = %name, "still running after SIGUSR1 — SIGKILL");
                let _ = self.signal_run(proc_arc, run, pid, "SIGKILL").await;
            }
            return;
        }
    }

    /// Signal `pid` if it is still `run`'s process. False when the run has
    /// ended (and nothing was sent).
    async fn signal_run(
        &self,
        proc_arc: &Arc<Mutex<ManagedProcess>>,
        run: u64,
        pid: Option<u32>,
        signal: &str,
    ) -> bool {
        let proc = proc_arc.lock().await;
        let Some(pid) = pid.filter(|_| proc.run == run && proc.pid == pid) else {
            return false;
        };
        if let Err(e) = send_signal(pid, signal) {
            warn!(process = %proc.config.name, pid, signal, error = %e, "signal failed");
        }
        true
    }

    async fn handle_exit(self: &Arc<Self>, name: &str, exit_code: Option<i32>) {
        let procs = self.processes.read().await;
        let proc_arc = match procs.get(name) {
            Some(p) => p.clone(),
            None => return,
        };
        drop(procs);

        let (
            failure_action,
            exit_action,
            restart_delay,
            max_restarts,
            window,
            restarts_in_window,
            no_restart,
            no_restart_action,
        ) = {
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
                is_no_restart(exit_code, &proc.config.no_restart_exit_codes),
                proc.config.on_no_restart.clone(),
            )
        };

        // An exit while stormd is stopping is the stop, not a crash — under a
        // supervisor or `timeout`, the signal often reaches the whole process
        // group, so the child dies before stormd's own kill does. Counting
        // that as a crash and scheduling a restart is noise in the last lines
        // anyone reads.
        if self.is_shutting_down() {
            proc_arc.lock().await.state = ProcessState::Stopped;
            self.stormlog.archive_run(name, false).await;
            info!(process = %name, code = ?exit_code, "process exited during shutdown");
            return;
        }

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

        // Close out this run's log file: renamed after the run, old runs pruned
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
            let mut detail = HashMap::new();
            if let Some(code) = exit_code {
                detail.insert("code".to_string(), code.into());
            }
            if no_restart {
                detail.insert("no_restart".to_string(), true.into());
            }
            self.event_bus
                .emit(EventKind::ProcessCrashed, Some(name.to_string()), detail)
                .await;
        }

        // The process said so itself: this exit is one a restart will not
        // fix (a config it cannot run on, usually). Restarting it would be
        // the loop that filled a node's console for hours — see stormd#2 —
        // so it is marked Failed, which every dashboard shows in red, and
        // left there. It does not count as a restart, because it was not one.
        if no_restart {
            {
                let mut proc = proc_arc.lock().await;
                proc.state = ProcessState::Failed;
            }
            error!(
                process = %name,
                code = ?exit_code,
                "process exited with a non-retryable code — not restarting"
            );
            match no_restart_action {
                NoRestartAction::Hold => {
                    info!(process = %name, "on_no_restart is 'hold' — container keeps running");
                }
                NoRestartAction::Fail => {
                    error!(process = %name, "on_no_restart is 'fail' — failing container");
                    *self.container_failed.write().await = true;
                    self.event_bus
                        .emit_simple(EventKind::ContainerFailing, None)
                        .await;
                }
            }
            return;
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
            tokio::time::sleep(cooloff(restart_delay, restarts_in_window)).await;
            self.wait_for_node_vars(name).await;
            if self.stand_down(&proc_arc, name).await {
                return;
            }
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
                    let wait = cooloff(restart_delay, restarts_in_window);
                    info!(
                        process = %name,
                        delay_secs = wait.as_secs(),
                        restarts = restarts_in_window,
                        "restarting process"
                    );
                    // Which attempt this is and how long it waits — the two
                    // facts a reader needs and the ones a bare "restarting"
                    // leaves them to count for themselves.
                    let mut detail = std::collections::HashMap::new();
                    detail.insert("restart".to_string(), restarts_in_window.into());
                    detail.insert("cooloff_secs".to_string(), wait.as_secs().into());
                    self.event_bus
                        .emit(
                            EventKind::ProcessRestarting,
                            Some(name.to_string()),
                            detail,
                        )
                        .await;
                    tokio::time::sleep(wait).await;
                    self.wait_for_node_vars(name).await;
                    if self.stand_down(&proc_arc, name).await {
                        return;
                    }
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
                    // The old run must be gone before the new one starts:
                    // a slow exit after SIGTERM would hold its port.
                    self.wait_stopped(name).await;
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

    /// Whether a process is healthy for its current run (stormd#44): running,
    /// and every check it has passed since this spawn — its ready_probe, its
    /// liveness probe, each declared API (`healthy`, probed after the
    /// spawn). With none of those, running for [`SETTLE_SECS`]. `Err` lists
    /// what it is still waiting on.
    fn health_of(&self, p: &ManagedProcess) -> Result<(), Vec<String>> {
        let mut waiting = Vec::new();
        if p.state != ProcessState::Running {
            return Err(vec![format!("not running ({:?})", p.state)]);
        }
        let started = p.started_at.unwrap_or_else(Utc::now);
        if p.config.ready_probe.is_some() && !p.ready {
            waiting.push("ready_probe has not passed".to_string());
        }
        if p.config.liveness.is_some() && p.liveness_passed_run != Some(p.run) {
            waiting.push(format!("liveness probe has not passed for this run ({} failures)", p.liveness_failures));
        }
        if !p.config.api.is_empty() {
            let seen = self.api_health.list();
            for a in &p.config.api {
                match seen.iter().find(|h| h.process == p.config.name && h.api == a.name) {
                    Some(h) if h.state == crate::apihealth::ApiState::Healthy && h.last_check.is_some_and(|t| t >= started) => {}
                    Some(h) => waiting.push(format!(
                        "api {} is {:?}{}",
                        a.name,
                        h.state,
                        h.last_error.as_deref().map(|e| format!(": {e}")).unwrap_or_default()
                    )),
                    None => waiting.push(format!("api {} not probed yet", a.name)),
                }
            }
        }
        let has_checks = p.config.ready_probe.is_some() || p.config.liveness.is_some() || !p.config.api.is_empty();
        if !has_checks && (Utc::now() - started).num_seconds() < SETTLE_SECS {
            waiting.push(format!("running less than {SETTLE_SECS} s"));
        }
        if waiting.is_empty() { Ok(()) } else { Err(waiting) }
    }

    /// Restart a process and wait until its new run is healthy, it ends, or
    /// `timeout` passes (stormd#44). On a timeout the process is left
    /// running: the caller decides.
    pub async fn restart_and_wait(self: &Arc<Self>, name: &str, timeout: Duration) -> anyhow::Result<WaitOutcome> {
        self.restart_process(name).await?;
        let proc_arc = self
            .processes
            .read()
            .await
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("process not found: {}", name))?;
        let run = proc_arc.lock().await.run;
        let started = tokio::time::Instant::now();
        loop {
            {
                let p = proc_arc.lock().await;
                if p.run != run || matches!(p.state, ProcessState::Stopped | ProcessState::Failed | ProcessState::Restarting) {
                    return Ok(WaitOutcome::Exited { run, exit_code: p.exit_code, state: p.state.clone() });
                }
                match self.health_of(&p) {
                    Ok(()) => return Ok(WaitOutcome::Healthy { run, waited: started.elapsed() }),
                    Err(w) if started.elapsed() >= timeout => return Ok(WaitOutcome::Timeout { run, waiting_on: w }),
                    Err(_) => {}
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    pub async fn get_status(&self, name: &str) -> anyhow::Result<ProcessStatus> {
        let procs = self.processes.read().await;
        let proc_arc = procs
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("process not found: {}", name))?;
        let proc = proc_arc.lock().await;
        let mut st = proc.status();
        st.healthy = self.health_of(&proc).is_ok();
        Ok(st)
    }

    pub async fn get_all_statuses(&self) -> Vec<ProcessStatus> {
        let procs = self.processes.read().await;
        let mut statuses = Vec::new();
        for p in procs.values() {
            let proc = p.lock().await;
            let mut st = proc.status();
            st.healthy = self.health_of(&proc).is_ok();
            statuses.push(st);
        }
        statuses.sort_by(|a, b| a.name.cmp(&b.name));
        statuses
    }

    /// Stop every running process, and start nothing more.
    ///
    /// **SIGTERM did not stop stormd** (stormd#17). The signal was received;
    /// then shutdown waited for the start order to finish, and the start
    /// order was parked on a dependency that would never be satisfied — a
    /// dependent of a one-shot that had failed. stormd sat there for ten
    /// hours under `timeout 10`, holding a build slot, with every later
    /// SIGTERM going to a handler nobody was reading any more. So this sets
    /// `shutting_down` first: the start order and any dependency wait give
    /// up, restarts stand down, and nothing new is spawned.
    ///
    /// Then it stops them in reverse dependency order, a tier at a time:
    /// SIGTERM, each process's `stop_timeout_secs`, then SIGKILL, and waits
    /// for the tier to be gone before the next, so the children are gone
    /// before stormd is (stormd#9). Safe to call twice; main does, once the
    /// start order has ended, to catch a process forked while the first call
    /// ran.
    pub async fn stop_all(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        // Dependents first: the apiserver flushes to fastetcd on SIGTERM, so
        // fastetcd stays up until the apiserver is gone (stormd#9).
        for tier in self.stop_tiers().await {
            let mut longest = 0;
            {
                let procs = self.processes.read().await;
                for name in &tier {
                    let Some(p) = procs.get(name) else { continue };
                    let mut proc = p.lock().await;
                    longest = longest.max(proc.config.stop_timeout_secs);
                    // A process marked Running a moment before its kill handle
                    // is stored is left Running, for the second call to catch.
                    if proc.state == ProcessState::Running {
                        if let Some(tx) = proc.kill_tx.take() {
                            proc.state = ProcessState::Stopping;
                            let _ = tx.send(());
                            info!(process = %name, "stopping process");
                        }
                    }
                }
            }
            let deadline = tokio::time::Instant::now() + Duration::from_secs(longest + STOP_SLACK_SECS);
            loop {
                let mut stopping = Vec::new();
                {
                    let procs = self.processes.read().await;
                    for name in &tier {
                        if let Some(p) = procs.get(name) {
                            if p.lock().await.state == ProcessState::Stopping {
                                stopping.push(name.clone());
                            }
                        }
                    }
                }
                if stopping.is_empty() {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    warn!(processes = ?stopping, "still stopping after SIGKILL — going on");
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }

    /// Processes grouped for shutdown, dependents first: a process's depth
    /// is one more than the deepest process it depends on, and the deepest
    /// tier stops first. A cycle or a missing name cannot loop: depth is
    /// capped at the number of processes.
    async fn stop_tiers(&self) -> Vec<Vec<String>> {
        let deps: HashMap<String, Vec<String>> = {
            let procs = self.processes.read().await;
            let mut deps = HashMap::new();
            for (name, p) in procs.iter() {
                deps.insert(name.clone(), p.lock().await.config.depends_on.clone());
            }
            deps
        };
        stop_tiers(&deps)
    }

    /// The longest shutdown can take with every process using its whole
    /// stop timeout, tier after tier. Main's watchdog allows this plus a
    /// margin before it exits regardless.
    pub async fn shutdown_budget(&self) -> Duration {
        let mut total = 0;
        let tiers = self.stop_tiers().await;
        let procs = self.processes.read().await;
        for tier in tiers {
            let mut longest = 0;
            for name in &tier {
                if let Some(p) = procs.get(name) {
                    longest = longest.max(p.lock().await.config.stop_timeout_secs);
                }
            }
            total += longest + STOP_SLACK_SECS;
        }
        Duration::from_secs(total)
    }

    /// Wait until a stop requested for `name` has landed: no longer
    /// `Stopping`, no pid. Bounded by its stop timeout plus slack.
    pub async fn wait_stopped(&self, name: &str) {
        let Some(p) = self.processes.read().await.get(name).cloned() else {
            return;
        };
        let timeout = p.lock().await.config.stop_timeout_secs + STOP_SLACK_SECS;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout);
        while tokio::time::Instant::now() < deadline {
            {
                let proc = p.lock().await;
                if proc.state != ProcessState::Stopping && proc.pid.is_none() {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        warn!(process = %name, "still not stopped after its stop timeout");
    }

    /// After a restart's cooloff: if stormd began shutting down meanwhile,
    /// leave the process stopped rather than start it into a container that
    /// is going away. `true` when it stood down.
    async fn stand_down(&self, proc_arc: &Arc<Mutex<ManagedProcess>>, name: &str) -> bool {
        if !self.is_shutting_down() {
            return false;
        }
        proc_arc.lock().await.state = ProcessState::Stopped;
        info!(process = %name, "shutting down — not restarting");
        true
    }

    /// Sleep, but wake early if shutdown begins.
    async fn sleep_unless_shutdown(&self, total: Duration) {
        let end = tokio::time::Instant::now() + total;
        while !self.is_shutting_down() {
            let now = tokio::time::Instant::now();
            if now >= end {
                return;
            }
            tokio::time::sleep((end - now).min(Duration::from_millis(250))).await;
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
            run: 0,
            liveness_tasks: Arc::new(AtomicUsize::new(0)),
            ready: config.ready_probe.is_none(),
            ready_at: None,
            liveness_passed_at: None,
            liveness_passed_run: None,
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
        send_signal(pid, signal)
    }
}

/// Those of `files` that do not exist (a broken symlink counts as missing).
fn missing_files(files: &[String]) -> Vec<&str> {
    files
        .iter()
        .filter(|f| !std::path::Path::new(f.as_str()).exists())
        .map(|f| f.as_str())
        .collect()
}

/// A process with no ready, liveness or API checks counts as healthy once it
/// has been running this long (stormd#44).
const SETTLE_SECS: i64 = 3;

/// How a `restart_and_wait` ended.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum WaitOutcome {
    Healthy { run: u64, #[serde(serialize_with = "ms")] waited: Duration },
    Timeout { run: u64, waiting_on: Vec<String> },
    Exited { run: u64, exit_code: Option<i32>, state: ProcessState },
}

fn ms<S: serde::Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u64(d.as_millis() as u64)
}

/// Beyond a stop timeout: SIGKILL landing and the monitor recording it.
const STOP_SLACK_SECS: u64 = 2;

/// Stop a child: SIGTERM, up to `timeout` for it to exit, then SIGKILL.
/// A zero timeout is SIGKILL at once. Returns the exit code, if it exited
/// with one (not by signal).
///
/// **Every stop used to be SIGKILL** (stormd#9): a supervised process could
/// not flush, close a WAL or deregister, whether stopped from the API, the
/// shell, a restart, the updater's pivot or shutdown.
async fn stop_child(
    child: &mut tokio::process::Child,
    pid: Option<u32>,
    timeout: Duration,
    name: &str,
) -> Option<i32> {
    if let (Some(pid), false) = (pid, timeout.is_zero()) {
        if send_signal(pid, "SIGTERM").is_ok() {
            match tokio::time::timeout(timeout, child.wait()).await {
                Ok(status) => return status.ok().and_then(|s| s.code()),
                Err(_) => warn!(
                    process = %name,
                    timeout_secs = timeout.as_secs(),
                    "still running after SIGTERM — SIGKILL"
                ),
            }
        }
    }
    let _ = child.kill().await;
    None
}

/// Shutdown tiers from each process's `depends_on`, deepest first.
fn stop_tiers(deps: &HashMap<String, Vec<String>>) -> Vec<Vec<String>> {
    fn depth(
        name: &str,
        deps: &HashMap<String, Vec<String>>,
        memo: &mut HashMap<String, usize>,
        budget: usize,
    ) -> usize {
        if let Some(d) = memo.get(name) {
            return *d;
        }
        if budget == 0 {
            return 0;
        }
        let d = deps
            .get(name)
            .map(|ds| {
                ds.iter()
                    .filter(|d| deps.contains_key(*d))
                    .map(|d| 1 + depth(d, deps, memo, budget - 1))
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        memo.insert(name.to_string(), d);
        d
    }
    let mut memo = HashMap::new();
    let mut tiers: Vec<Vec<String>> = Vec::new();
    let mut names: Vec<&String> = deps.keys().collect();
    names.sort();
    for name in names {
        let d = depth(name, deps, &mut memo, deps.len());
        if tiers.len() <= d {
            tiers.resize(d + 1, Vec::new());
        }
        tiers[d].push(name.clone());
    }
    tiers.reverse();
    tiers.retain(|t| !t.is_empty());
    tiers
}

fn send_signal(pid: u32, signal: &str) -> anyhow::Result<()> {
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

/// Counts a live task for as long as it is held (dropped on abort too).
struct TaskCount(Arc<AtomicUsize>);

impl TaskCount {
    fn enter(count: Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        TaskCount(count)
    }
}

impl Drop for TaskCount {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Aborts tasks when dropped.
struct AbortOnDrop(Vec<tokio::task::JoinHandle<()>>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        for h in self.0.drain(..) {
            h.abort();
        }
    }
}


/// The client probes are made with. Built once.
///
/// Certificate verification is off deliberately; see `execute_probe`.
/// Run a readiness probe once.
///
/// Shares the liveness probe's client, and therefore its decision not to
/// verify certificates: the question is whether the thing answers, and the
/// supervisor already knows what it started.
async fn execute_ready_probe(probe: &crate::config::ReadyProbe) -> bool {
    use crate::config::ReadyProbe;
    match probe {
        ReadyProbe::Http { url, .. } => {
            let Some(client) = probe_client() else {
                return false;
            };
            match tokio::time::timeout(Duration::from_secs(5), client.get(url).send()).await {
                Ok(Ok(resp)) => resp.status().is_success() || resp.status().is_redirection(),
                _ => false,
            }
        }
        ReadyProbe::Tcp { port, .. } => {
            let addr = format!("127.0.0.1:{port}");
            matches!(
                tokio::time::timeout(
                    Duration::from_secs(5),
                    tokio::net::TcpStream::connect(&addr)
                )
                .await,
                Ok(Ok(_))
            )
        }
        ReadyProbe::Exec { command, .. } => {
            let mut parts = command.split_whitespace();
            let Some(bin) = parts.next() else {
                return false;
            };
            matches!(
                tokio::process::Command::new(bin)
                    .args(parts)
                    .status()
                    .await,
                Ok(st) if st.success()
            )
        }
    }
}

fn probe_client() -> Option<&'static reqwest::Client> {
    static CLIENT: std::sync::OnceLock<Option<reqwest::Client>> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .danger_accept_invalid_certs(true)
                .build()
                .ok()
        })
        .as_ref()
}

async fn execute_probe(probe: &LivenessProbe) -> bool {
    let timeout = Duration::from_secs(probe.timeout_secs);
    match &probe.probe {
        ProbeType::Http { url } => {
            // **A liveness probe does not verify the certificate.**
            //
            // The question is whether this process is alive on loopback, not
            // whether it is the server it claims to be — and the answer to the
            // second is already known, because the supervisor started it. A
            // verifying client turns a healthy process behind a self-signed
            // certificate into a dead one: the apiserver here serves a cert
            // its own node minted, every probe failed, and stormd killed it on
            // schedule roughly every twenty-five seconds. Nothing in the log
            // said "certificate" — the process was simply "killed by signal".
            //
            // Upstream's kubelet skips verification on httpGet probes for the
            // same reason.
            let client = match probe_client() {
                Some(c) => c,
                None => return false,
            };
            match tokio::time::timeout(timeout, client.get(url).send()).await {
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

/// How long to wait before starting a process again.
///
/// `base`, doubled per restart already made inside the window, capped. The
/// count comes from the restart window, so it falls back to nothing on its own
/// once a process has stayed up — there is no separate "healthy" timer to keep
/// in step with it.
///
/// A flat delay is what was here, and against a process that will never start
/// it is simply a loop: the apiserver in this stack restarted once a second
/// for as long as it was left, which is a core spent and a log filled with one
/// line. Capped rather than given up on, because a supervised process is part
/// of what the container is — the answer to "it keeps failing" is to keep
/// trying and keep saying so, not to stop quietly and look healthy.
fn cooloff(base: u64, restarts_in_window: u32) -> std::time::Duration {
    const CEILING: u64 = 30;
    let base = base.max(1);
    let shift = restarts_in_window.saturating_sub(1).min(16);
    let secs = base.saturating_mul(1u64 << shift).min(CEILING);
    std::time::Duration::from_secs(secs)
}

/// Whether a process satisfies a `depends_on` naming it.
///
/// A long-running process satisfies once it is running and ready — and a
/// process with no `ready_probe` is ready the moment it is spawned.
///
/// **A one-shot was satisfied at spawn.** For a one-shot (`on_exit = "stop"`)
/// with no probe, "running and ready" is true while it is still doing the job
/// its dependents wait for; stormcert-node-admin ran and failed before
/// stormcert-sa had written the key it signs with (stormd#16). So a one-shot
/// without a probe satisfies only once it has *finished* — stopped, having
/// exited 0. One with a probe has said what "ready" means and keeps
/// satisfying on it.
///
/// **A one-shot was unsatisfiable.** Waiting only for `Running` missed a task
/// that does its job and exits inside one poll interval, and the dependent
/// waited forever for a state the process would never be in again. Hence
/// `Stopped` counts — but only for a one-shot, and only after a clean exit. A
/// process meant to keep running and found stopped has satisfied nothing; nor
/// has a one-shot that failed under `on_failure = "ignore"` (also `Stopped`)
/// or was stopped by hand (killed, no code).
fn dependency_satisfied(
    state: &ProcessState,
    ready: bool,
    has_ready_probe: bool,
    on_exit: &crate::config::ExitAction,
    exit_code: Option<i32>,
) -> bool {
    let one_shot = *on_exit == crate::config::ExitAction::Stop;
    let finished = *state == ProcessState::Stopped && exit_code == Some(0);
    if one_shot {
        finished || (has_ready_probe && *state == ProcessState::Running && ready)
    } else {
        *state == ProcessState::Running && ready
    }
}

/// What a process gets on top of the environment it inherits from stormd.
///
/// `env` always wins: it is set over whatever stormd inherited. `env_default`
/// fills only the gaps: an entry whose key stormd inherited (even an empty
/// value) is left alone, so the node's own override (stormpump's
/// Why `cfg` cannot be spawned with `vars`: a `${NODE_*}` left in an
/// expanded argument or in an environment value it would get. `None` when
/// nothing is missing.
fn node_vars_missing(cfg: &ProcessConfig, vars: &HashMap<String, String>) -> Option<String> {
    let mut missing: Vec<&'static str> = Vec::new();
    let args = cfg.args.iter().map(|a| crate::nodevars::expand(a, vars));
    let env = process_env(&cfg.env, &cfg.env_default, |k| std::env::var_os(k).is_some(), vars)
        .into_iter()
        .map(|(_, v)| v);
    for s in args.chain(env) {
        for n in crate::nodevars::unexpanded(&s) {
            if !missing.contains(&n) {
                missing.push(n);
            }
        }
    }
    let first = missing.first()?;
    let names = missing.iter().map(|n| format!("${{{n}}}")).collect::<Vec<_>>().join(", ");
    Some(format!(
        "process '{}' needs {names}, and {}",
        cfg.name,
        crate::nodevars::why_missing(first)
    ))
}

/// `env.d/<spec>`, which is stormd's inherited environment) beats the golden's
/// default. A key in both `env` and `env_default` takes `env`'s value. Both
/// are expanded like `args`.
fn process_env(
    env: &HashMap<String, String>,
    env_default: &HashMap<String, String>,
    inherited: impl Fn(&str) -> bool,
    vars: &HashMap<String, String>,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = env_default
        .iter()
        .filter(|(k, _)| !env.contains_key(*k) && !inherited(k))
        .map(|(k, v)| (k.clone(), crate::nodevars::expand(v, vars)))
        .collect();
    out.extend(
        env.iter()
            .map(|(k, v)| (k.clone(), crate::nodevars::expand(v, vars))),
    );
    out
}

#[cfg(test)]
mod env_tests {
    use super::process_env;
    use std::collections::HashMap;

    fn m(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn run(
        env: &[(&str, &str)],
        def: &[(&str, &str)],
        inherited: &[&str],
    ) -> HashMap<String, String> {
        let vars = m(&[("NODE_IP", "10.0.0.7")]);
        process_env(&m(env), &m(def), |k| inherited.contains(&k), &vars)
            .into_iter()
            .collect()
    }

    #[test]
    fn a_default_applies_when_not_inherited() {
        let got = run(&[], &[("APISERVER_URL", "https://${NODE_IP}:6443")], &[]);
        assert_eq!(got["APISERVER_URL"], "https://10.0.0.7:6443");
    }

    #[test]
    fn an_inherited_value_beats_the_default() {
        let got = run(&[], &[("FASTETCD_DATA_DIR", "/data/fastetcd")], &["FASTETCD_DATA_DIR"]);
        assert!(!got.contains_key("FASTETCD_DATA_DIR"), "left to the inherited value");
    }

    #[test]
    fn env_beats_both_and_is_always_set() {
        let got = run(&[("K", "env")], &[("K", "default")], &["K"]);
        assert_eq!(got["K"], "env");
        assert_eq!(got.len(), 1);
    }
}

#[cfg(test)]
mod dependency_tests {
    use super::{dependency_satisfied, ProcessState};
    use crate::config::ExitAction;

    #[test]
    fn a_one_shot_without_a_probe_satisfies_only_when_finished() {
        // Spawned and "ready" (no probe) but still working: not yet.
        assert!(!dependency_satisfied(&ProcessState::Running, true, false, &ExitAction::Stop, None));
        assert!(!dependency_satisfied(&ProcessState::Starting, true, false, &ExitAction::Stop, None));
        // Exited 0 and stopped: done.
        assert!(dependency_satisfied(&ProcessState::Stopped, true, false, &ExitAction::Stop, Some(0)));
    }

    #[test]
    fn a_one_shot_that_did_not_finish_cleanly_does_not_satisfy() {
        // on_failure = "ignore" leaves it Stopped with its code.
        assert!(!dependency_satisfied(&ProcessState::Stopped, true, false, &ExitAction::Stop, Some(1)));
        // Stopped by hand: killed, no code.
        assert!(!dependency_satisfied(&ProcessState::Stopped, true, false, &ExitAction::Stop, None));
        assert!(!dependency_satisfied(&ProcessState::Failed, true, false, &ExitAction::Stop, Some(78)));
    }

    #[test]
    fn a_one_shot_with_a_probe_satisfies_on_the_probe() {
        assert!(!dependency_satisfied(&ProcessState::Running, false, true, &ExitAction::Stop, None));
        assert!(dependency_satisfied(&ProcessState::Running, true, true, &ExitAction::Stop, None));
        assert!(dependency_satisfied(&ProcessState::Stopped, false, true, &ExitAction::Stop, Some(0)));
    }

    #[test]
    fn a_long_running_process_satisfies_when_running_and_ready() {
        assert!(dependency_satisfied(&ProcessState::Running, true, false, &ExitAction::Restart, None));
        assert!(!dependency_satisfied(&ProcessState::Running, false, true, &ExitAction::Restart, None));
        // Stopped, even cleanly, is not what it was meant to be.
        assert!(!dependency_satisfied(&ProcessState::Stopped, true, false, &ExitAction::Restart, Some(0)));
    }
}

/// Whether an exit is one the process has declared not worth retrying. A
/// clean exit is never one, whatever the list says, and a death by signal
/// has no code to match — the process did not choose it.
fn is_no_restart(exit_code: Option<i32>, codes: &[i32]) -> bool {
    matches!(exit_code, Some(c) if c != 0 && codes.contains(&c))
}

#[cfg(test)]
mod no_restart_tests {
    use super::is_no_restart;

    #[test]
    fn a_listed_code_is_not_retried() {
        assert!(is_no_restart(Some(78), &[78]));
        assert!(is_no_restart(Some(64), &[64, 78]));
    }

    #[test]
    fn everything_else_is() {
        assert!(!is_no_restart(Some(1), &[78]));
        assert!(!is_no_restart(Some(78), &[]));
        // Killed by a signal: no code, nothing the process said.
        assert!(!is_no_restart(None, &[78]));
        // Success is success even if someone lists it.
        assert!(!is_no_restart(Some(0), &[0]));
    }

    #[test]
    fn the_config_keys_parse_with_their_defaults() {
        let c: crate::config::ProcessConfig =
            toml::from_str("name = \"a\"\ncommand = \"/a\"\n").unwrap();
        assert!(c.no_restart_exit_codes.is_empty());
        assert_eq!(c.on_no_restart, crate::config::NoRestartAction::Hold);
        let c: crate::config::ProcessConfig = toml::from_str(
            "name = \"a\"\ncommand = \"/a\"\nno_restart_exit_codes = [78, 64]\non_no_restart = \"fail\"\n",
        )
        .unwrap();
        assert_eq!(c.no_restart_exit_codes, vec![78, 64]);
        assert_eq!(c.on_no_restart, crate::config::NoRestartAction::Fail);
    }
}

#[cfg(test)]
mod cooloff_tests {
    use super::cooloff;
    use std::time::Duration;

    #[test]
    fn it_escalates_and_settles() {
        // The first is quick, because a crash on start is usually a race with
        // something that has since come up.
        assert_eq!(cooloff(1, 1), Duration::from_secs(1));
        assert_eq!(cooloff(1, 2), Duration::from_secs(2));
        assert_eq!(cooloff(1, 3), Duration::from_secs(4));
        assert_eq!(cooloff(1, 5), Duration::from_secs(16));
        // And settles, rather than growing without bound or giving up.
        assert_eq!(cooloff(1, 6), Duration::from_secs(30));
        assert_eq!(cooloff(1, 500), Duration::from_secs(30));
    }

    #[test]
    fn a_longer_base_is_respected_and_still_capped() {
        assert_eq!(cooloff(5, 1), Duration::from_secs(5));
        assert_eq!(cooloff(5, 3), Duration::from_secs(20));
        assert_eq!(cooloff(5, 4), Duration::from_secs(30));
    }

    #[test]
    fn a_zero_base_still_waits() {
        // A configured zero would be the loop this exists to stop.
        assert_eq!(cooloff(0, 1), Duration::from_secs(1));
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::Supervisor;
    use std::sync::Arc;
    use std::time::Duration;

    fn supervisor() -> Arc<Supervisor> {
        let cfg: crate::config::Config = toml::from_str(
            "[general]\nname = \"t\"\nlog_dir = \"/nonexistent/stormd-test\"\n\
             [stormlog.mcast]\ngroup = \"off\"\n",
        )
        .unwrap();
        let bus = Arc::new(crate::events::EventBus::new(cfg.events.clone(), "t".into()));
        let log = Arc::new(stormlog::StormLog::new(cfg.stormlog.clone(), "t"));
        Arc::new(Supervisor::new(log, bus))
    }

    /// stormd#17: the start order parked on a dependency that can never be
    /// satisfied must end when shutdown begins, not hold stormd forever.
    #[tokio::test]
    async fn stopping_ends_a_start_order_parked_on_a_dependency() {
        let sup = supervisor();
        let cfg: crate::config::ProcessConfig = toml::from_str(
            "name = \"held\"\ncommand = \"/bin/true\"\ndepends_on = [\"never\"]\n",
        )
        .unwrap();
        let s = sup.clone();
        let start = tokio::spawn(async move { s.start_all(&[cfg]).await });
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(!start.is_finished(), "should be waiting on its dependency");

        sup.stop_all().await;
        let r = tokio::time::timeout(Duration::from_secs(2), start).await;
        assert!(r.is_ok(), "start order still waiting after stop_all");
        assert!(r.unwrap().unwrap().is_ok());
        // And nothing starts afterwards.
        assert!(sup.start_process("held").await.is_err());
    }
}

#[cfg(test)]
mod exit_handler_tests {
    use super::{ProcessState, Supervisor};
    use std::sync::Arc;
    use std::time::Duration;

    /// stormd#22: a process in its restart cooloff must not delay handling
    /// another process's exit.
    #[tokio::test]
    async fn a_cooloff_does_not_hold_up_another_exit() {
        let dir = std::env::temp_dir().join(format!("stormd-exit-test-{}", std::process::id()));
        let cfg: crate::config::Config = toml::from_str(&format!(
            "[general]\nname = \"t\"\nlog_dir = \"{}\"\n[stormlog.mcast]\ngroup = \"off\"\n",
            dir.display()
        ))
        .unwrap();
        let mut log_cfg = cfg.stormlog.clone();
        log_cfg.file.log_dir = dir.clone();
        let bus = Arc::new(crate::events::EventBus::new(cfg.events.clone(), "t".into()));
        let log = Arc::new(stormlog::StormLog::new(log_cfg, "t"));
        let sup = Arc::new(Supervisor::new(log, bus));
        let h = sup.clone();
        tokio::spawn(async move { h.run_exit_handler().await });

        let procs: Vec<crate::config::ProcessConfig> = [
            // Crashes at once, then waits out a 5 s cooloff.
            "name = \"slow\"\ncommand = \"/bin/sh\"\nargs = [\"-c\", \"exit 1\"]\nrestart_delay_secs = 5\n",
            // Crashes half a second later, while `slow` is cooling off.
            "name = \"quick\"\ncommand = \"/bin/sh\"\nargs = [\"-c\", \"sleep 0.5; exit 1\"]\non_failure = \"ignore\"\non_exit = \"stop\"\n",
        ]
        .iter()
        .map(|t| toml::from_str(t).unwrap())
        .collect();
        sup.start_all(&procs).await.unwrap();

        let mut handled = false;
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let q = sup.get_status("quick").await.unwrap();
            if q.state == ProcessState::Stopped && q.exit_code == Some(1) {
                handled = true;
                break;
            }
        }
        let slow = sup.get_status("slow").await.unwrap();
        sup.stop_all().await;
        let _ = std::fs::remove_dir_all(&dir);
        assert!(handled, "quick's exit was not handled within 2 s (slow is {:?})", slow.state);
        assert_eq!(slow.state, ProcessState::Restarting, "slow should still be in its cooloff");
    }
}

#[cfg(test)]
mod liveness_tests {
    use super::{ProcessState, Supervisor};
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    async fn status(sup: &Supervisor, name: &str) -> (ProcessState, Option<u32>, u32, u32, usize) {
        let s = sup.get_status(name).await.unwrap();
        let tasks = {
            let procs = sup.processes.read().await;
            let p = procs.get(name).unwrap().lock().await;
            p.liveness_tasks.load(Ordering::SeqCst)
        };
        (s.state, s.pid, s.restarts, s.liveness_failures, tasks)
    }

    /// stormd#45: run 1 crashes inside its `initial_delay_secs`, and its
    /// liveness task must not wake on run 2 and probe it before run 2's own
    /// delay. Once the probe passes, run 2 is left running; after a restart
    /// there is still exactly one liveness task.
    #[tokio::test]
    async fn a_liveness_task_ends_with_its_run() {
        let dir = std::env::temp_dir().join(format!("stormd-liveness-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg: crate::config::Config = toml::from_str(&format!(
            "[general]\nname = \"t\"\nlog_dir = \"{}\"\n[stormlog.mcast]\ngroup = \"off\"\n",
            dir.display()
        ))
        .unwrap();
        let mut log_cfg = cfg.stormlog.clone();
        log_cfg.file.log_dir = dir.clone();
        let bus = Arc::new(crate::events::EventBus::new(cfg.events.clone(), "t".into()));
        let log = Arc::new(stormlog::StormLog::new(log_cfg, "t"));
        let sup = Arc::new(Supervisor::new(log, bus));
        let h = sup.clone();
        tokio::spawn(async move { h.run_exit_handler().await });

        // A port nothing listens on yet: the probe fails until we listen.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let once = dir.join("once");
        // Run 1 exits 1 after 0.5 s; later runs stay up.
        let proc: crate::config::ProcessConfig = toml::from_str(&format!(
            "name = \"p\"\ncommand = \"/bin/sh\"\n\
             args = [\"-c\", \"if [ -e {o} ]; then exec sleep 60; else touch {o}; sleep 0.5; exit 1; fi\"]\n\
             restart_delay_secs = 1\n\
             [liveness]\ntype = \"tcp\"\nport = {port}\ninitial_delay_secs = 3\ninterval_secs = 1\n\
             failure_threshold = 1\ntimeout_secs = 1\n",
            o = once.display()
        ))
        .unwrap();
        let t0 = Instant::now();
        sup.start_all(&[proc]).await.unwrap();

        // Wait for run 2.
        let mut run2 = None;
        while t0.elapsed() < Duration::from_secs(5) {
            let (state, pid, restarts, _, _) = status(&sup, "p").await;
            if state == ProcessState::Running && restarts == 1 {
                run2 = pid;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let run2_at = t0.elapsed();
        let run2 = run2.expect("no second run within 5 s");

        // Run 1's task would wake at 3 + 1 = 4 s; run 2's own first probe is
        // 4 s after run 2 started. Look just before run 2's first probe.
        tokio::time::sleep((run2_at + Duration::from_millis(3500)).saturating_sub(t0.elapsed())).await;
        let (state, pid, restarts, failures, tasks) = status(&sup, "p").await;
        assert!(t0.elapsed() > Duration::from_millis(4300), "test timing: looked too early");
        assert_eq!((state.clone(), pid, restarts), (ProcessState::Running, Some(run2), 1),
            "run 2 was probed (and killed) before its own initial delay");
        assert_eq!(failures, 0, "run 2 inherited run 1's liveness failures");
        assert_eq!(tasks, 1, "liveness tasks alive for run 2");

        // The probe now passes: run 2 stays up past its delay and a few probes.
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap();
        tokio::spawn(async move { loop { let _ = listener.accept().await; } });
        tokio::time::sleep(Duration::from_secs(4)).await;
        let (state, pid, restarts, failures, tasks) = status(&sup, "p").await;
        assert_eq!((state, pid, restarts, failures, tasks), (ProcessState::Running, Some(run2), 1, 0, 1));

        // A second restart still leaves exactly one task.
        sup.restart_process("p").await.unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        let (state, pid, _, _, tasks) = status(&sup, "p").await;
        sup.stop_all().await;
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(state, ProcessState::Running);
        assert_ne!(pid, Some(run2), "restart_process did not start a new run");
        assert_eq!(tasks, 1, "liveness tasks alive after a second restart");
    }
}

#[cfg(test)]
mod stop_tests {
    use super::{stop_tiers, ProcessState, Supervisor};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn supervisor(label: &str) -> (Arc<Supervisor>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("stormd-stop-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg: crate::config::Config = toml::from_str(&format!(
            "[general]\nname = \"t\"\nlog_dir = \"{}\"\n[stormlog.mcast]\ngroup = \"off\"\n",
            dir.display()
        ))
        .unwrap();
        let mut log_cfg = cfg.stormlog.clone();
        log_cfg.file.log_dir = dir.clone();
        let bus = Arc::new(crate::events::EventBus::new(cfg.events.clone(), "t".into()));
        let log = Arc::new(stormlog::StormLog::new(log_cfg, "t"));
        (Arc::new(Supervisor::new(log, bus)), dir)
    }

    fn sh(name: &str, script: &str, extra: &str) -> crate::config::ProcessConfig {
        toml::from_str(&format!(
            "name = \"{name}\"\ncommand = \"/bin/sh\"\nargs = [\"-c\", '''{script}''']\n{extra}\n"
        ))
        .unwrap()
    }

    /// Stop `name` and time it until the stop has landed.
    async fn stop(sup: &Supervisor, name: &str) -> Duration {
        let t = Instant::now();
        sup.stop_process(name).await.unwrap();
        sup.wait_stopped(name).await;
        t.elapsed()
    }

    /// Wait for a process's start script to have got as far as `marker`.
    async fn wait_file(path: &std::path::Path) {
        for _ in 0..100 {
            if path.exists() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("{} never appeared", path.display());
    }

    /// stormd#9: a stop is SIGTERM first — the process's handler runs, and
    /// its exit code is recorded.
    #[tokio::test]
    async fn stop_sends_sigterm_and_records_the_exit() {
        let (sup, dir) = supervisor("term");
        let up = dir.join("up");
        let bye = dir.join("bye");
        let p = sh(
            "p",
            &format!(
                "trap 'echo bye > {b}; exit 7' TERM; touch {u}; while :; do sleep 0.1; done",
                b = bye.display(),
                u = up.display()
            ),
            "",
        );
        sup.start_all(&[p]).await.unwrap();
        wait_file(&up).await;
        let took = stop(&sup, "p").await;
        let s = sup.get_status("p").await.unwrap();
        let said_bye = bye.exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(said_bye, "the TERM handler did not run");
        assert_eq!(s.state, ProcessState::Stopped);
        assert_eq!(s.exit_code, Some(7), "the TERM handler's exit code");
        assert!(took < Duration::from_secs(3), "took {took:?}");
    }

    /// A process that ignores SIGTERM is SIGKILLed after its stop timeout.
    #[tokio::test]
    async fn sigterm_ignored_is_sigkill_after_the_timeout() {
        let (sup, dir) = supervisor("ignore");
        let up = dir.join("up");
        let p = sh(
            "p",
            &format!("trap '' TERM; touch {}; while :; do sleep 0.1; done", up.display()),
            "stop_timeout_secs = 1",
        );
        sup.start_all(&[p]).await.unwrap();
        wait_file(&up).await;
        let took = stop(&sup, "p").await;
        let s = sup.get_status("p").await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(s.state, ProcessState::Stopped);
        assert_eq!(s.pid, None);
        assert_eq!(s.exit_code, None, "killed by signal: no exit code");
        assert!(took >= Duration::from_millis(900), "SIGKILL before the timeout: {took:?}");
        assert!(took < Duration::from_secs(4), "took {took:?}");
    }

    /// `stop_timeout_secs = 0` is SIGKILL at once: the TERM handler never runs.
    #[tokio::test]
    async fn zero_timeout_is_sigkill_at_once() {
        let (sup, dir) = supervisor("zero");
        let up = dir.join("up");
        let bye = dir.join("bye");
        let p = sh(
            "p",
            &format!(
                "trap 'touch {b}; exit 0' TERM; touch {u}; while :; do sleep 0.1; done",
                b = bye.display(),
                u = up.display()
            ),
            "stop_timeout_secs = 0",
        );
        sup.start_all(&[p]).await.unwrap();
        wait_file(&up).await;
        stop(&sup, "p").await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let ran = bye.exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!ran, "the TERM handler ran: it was sent SIGTERM");
    }

    /// Shutdown stops a dependent, and waits for it, before what it depends on.
    #[tokio::test]
    async fn stop_all_stops_dependents_first() {
        let (sup, dir) = supervisor("order");
        let alive = dir.join("app-alive");
        let verdict = dir.join("verdict");
        let db = sh(
            "db",
            &format!(
                "trap 'if [ -e {a} ]; then echo early > {v}; else echo late > {v}; fi; exit 0' TERM; \
                 while :; do sleep 0.1; done",
                a = alive.display(),
                v = verdict.display()
            ),
            "",
        );
        let app = sh(
            "app",
            &format!(
                "trap 'sleep 0.5; rm -f {a}; exit 0' TERM; touch {a}; while :; do sleep 0.1; done",
                a = alive.display()
            ),
            "depends_on = [\"db\"]",
        );
        sup.start_all(&[db, app]).await.unwrap();
        wait_file(&alive).await;
        sup.stop_all().await;
        let v = std::fs::read_to_string(&verdict).unwrap_or_default();
        let db_s = sup.get_status("db").await.unwrap();
        let app_s = sup.get_status("app").await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(v.trim(), "late", "db got SIGTERM while app was still alive");
        assert_eq!((db_s.state, app_s.state), (ProcessState::Stopped, ProcessState::Stopped));
    }

    #[test]
    fn tiers_put_dependents_first_and_survive_cycles() {
        let deps: HashMap<String, Vec<String>> = [
            ("etcd", vec![]),
            ("apiserver", vec!["etcd"]),
            ("scheduler", vec!["apiserver"]),
            ("lone", vec!["missing"]),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.into_iter().map(String::from).collect()))
        .collect();
        assert_eq!(
            stop_tiers(&deps),
            vec![vec!["scheduler".to_string()], vec!["apiserver".into()], vec!["etcd".into(), "lone".into()]]
        );

        let cycle: HashMap<String, Vec<String>> =
            [("a", vec!["b"]), ("b", vec!["a"])]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.into_iter().map(String::from).collect()))
                .collect();
        let tiers = stop_tiers(&cycle);
        assert_eq!(tiers.iter().map(|t| t.len()).sum::<usize>(), 2, "each process once: {tiers:?}");
    }
}

#[cfg(test)]
mod wait_for_files_tests {
    use super::{missing_files, ProcessState, Supervisor};
    use std::sync::Arc;
    use std::time::Duration;

    fn supervisor(label: &str) -> (Arc<Supervisor>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("stormd-files-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg: crate::config::Config = toml::from_str(&format!(
            "[general]\nname = \"t\"\nlog_dir = \"{}\"\n[stormlog.mcast]\ngroup = \"off\"\n",
            dir.display()
        ))
        .unwrap();
        let mut log_cfg = cfg.stormlog.clone();
        log_cfg.file.log_dir = dir.clone();
        let bus = Arc::new(crate::events::EventBus::new(cfg.events.clone(), "t".into()));
        let log = Arc::new(stormlog::StormLog::new(log_cfg, "t"));
        (Arc::new(Supervisor::new(log, bus)), dir)
    }

    #[test]
    fn missing_lists_only_what_is_not_there() {
        let dir = std::env::temp_dir().join(format!("stormd-missing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let here = dir.join("here");
        std::fs::write(&here, b"x").unwrap();
        let dangling = dir.join("dangling");
        let _ = std::os::unix::fs::symlink(dir.join("nowhere"), &dangling);
        let files: Vec<String> = [&here, &dir.join("absent"), &dangling]
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        let missing: Vec<String> = missing_files(&files).into_iter().map(String::from).collect();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(missing, vec![files[1].clone(), files[2].clone()]);
    }

    /// stormd#38: the process is not spawned until its files exist, and is
    /// then started once — no crash, no restart, no cool-off.
    #[tokio::test]
    async fn first_start_waits_for_the_files() {
        let (sup, dir) = supervisor("wait");
        let crt = dir.join("fastetcd.crt");
        let key = dir.join("fastetcd.key");
        let p: crate::config::ProcessConfig = toml::from_str(&format!(
            "name = \"etcd\"\ncommand = \"/bin/sh\"\nargs = [\"-c\", \"test -e {c} && test -e {k} && exec sleep 30; exit 1\"]\n\
             wait_for_files = [\"{c}\", \"{k}\"]\n",
            c = crt.display(),
            k = key.display()
        ))
        .unwrap();
        let s = sup.clone();
        let start = tokio::spawn(async move { s.start_all(&[p]).await });

        std::fs::write(&crt, b"crt").unwrap();
        tokio::time::sleep(Duration::from_millis(700)).await;
        let held = sup.get_status("etcd").await.unwrap();
        assert_eq!(held.state, ProcessState::Pending, "started with the key still missing");
        assert!(!start.is_finished());

        std::fs::write(&key, b"key").unwrap();
        tokio::time::timeout(Duration::from_secs(2), start).await.expect("start order still waiting").unwrap().unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let s = sup.get_status("etcd").await.unwrap();
        sup.stop_all().await;
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!((s.state, s.restarts, s.crashes), (ProcessState::Running, 0, 0));
    }

    /// Shutdown ends the wait, like a `depends_on` wait (stormd#17).
    #[tokio::test]
    async fn shutdown_ends_the_wait() {
        let (sup, dir) = supervisor("shutdown");
        let p: crate::config::ProcessConfig = toml::from_str(&format!(
            "name = \"p\"\ncommand = \"/bin/true\"\nwait_for_files = [\"{}\"]\n",
            dir.join("never").display()
        ))
        .unwrap();
        let s = sup.clone();
        let start = tokio::spawn(async move { s.start_all(&[p]).await });
        tokio::time::sleep(Duration::from_millis(400)).await;
        sup.stop_all().await;
        let r = tokio::time::timeout(Duration::from_secs(2), start).await;
        let st = sup.get_status("p").await.unwrap().state;
        let _ = std::fs::remove_dir_all(&dir);
        assert!(r.is_ok(), "start order still waiting after stop_all");
        assert_eq!(st, ProcessState::Pending, "started while shutting down");
    }
}

#[cfg(test)]
mod node_vars_tests {
    use super::node_vars_missing;
    use std::collections::HashMap;

    fn cfg(t: &str) -> crate::config::ProcessConfig {
        toml::from_str(&format!("name = \"stormcert-init\"\ncommand = \"/bin/x\"\n{t}")).unwrap()
    }

    fn with_ip() -> HashMap<String, String> {
        HashMap::from([("NODE_IP".to_string(), "192.168.8.104".to_string()), ("NODE_NAME".into(), "n1".into())])
    }

    fn no_ip() -> HashMap<String, String> {
        HashMap::from([("NODE_NAME".to_string(), "n1".to_string())])
    }

    /// stormd#3: the argument stormcert-init failed on, with no address.
    #[test]
    fn an_unexpanded_node_ip_in_args_is_named_with_the_reason() {
        let c = cfg("args = [\"--ip\", \"10.96.0.1,${NODE_IP},127.0.0.1\"]\n");
        let why = node_vars_missing(&c, &no_ip()).expect("should refuse");
        assert_eq!(
            why,
            "process 'stormcert-init' needs ${NODE_IP}, and this node has no address on any interface (no route off the node)"
        );
        assert_eq!(node_vars_missing(&c, &with_ip()), None);
    }

    #[test]
    fn env_and_env_default_values_count_but_only_when_applied() {
        let c = cfg("env = { APISERVER_URL = \"https://${NODE_IP}:6443\" }\n");
        assert!(node_vars_missing(&c, &no_ip()).is_some());

        // An env_default stormd inherited is not applied, so not checked.
        std::env::set_var("STORMD_TEST_3_INHERITED", "https://10.0.0.1:6443");
        let c = cfg("env_default = { STORMD_TEST_3_INHERITED = \"https://${NODE_IP}:6443\" }\n");
        assert_eq!(node_vars_missing(&c, &no_ip()), None);
        let c = cfg("env_default = { STORMD_TEST_3_NOT_INHERITED = \"https://${NODE_IP}:6443\" }\n");
        assert!(node_vars_missing(&c, &no_ip()).is_some());
    }

    #[test]
    fn other_names_are_the_process_own() {
        let c = cfg("args = [\"-c\", \"echo ${HOME} $NODE_IP ${NODE_IPV6}\"]\nenv = { X = \"${PATH}\" }\n");
        assert_eq!(node_vars_missing(&c, &no_ip()), None);
    }

    #[test]
    fn several_missing_names_are_all_named() {
        let c = cfg("args = [\"${NODE_NAME}\", \"${NODE_IP}\"]\n");
        let why = node_vars_missing(&c, &HashMap::new()).unwrap();
        assert!(why.contains("needs ${NODE_NAME}, ${NODE_IP}"), "{why}");
    }
}

#[cfg(test)]
mod api_health_tests {
    use super::{ProcessState, Supervisor};
    use crate::apihealth::ApiState;
    use std::sync::Arc;
    use std::time::Duration;

    async fn supervisor(label: &str) -> (Arc<Supervisor>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("stormd-apiht-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg: crate::config::Config = toml::from_str(&format!(
            "[general]\nname = \"t\"\nlog_dir = \"{}\"\n[stormlog.mcast]\ngroup = \"off\"\n",
            dir.display()
        ))
        .unwrap();
        let mut log_cfg = cfg.stormlog.clone();
        log_cfg.file.log_dir = dir.clone();
        let bus = Arc::new(crate::events::EventBus::new(cfg.events.clone(), "t".into()));
        let log = Arc::new(stormlog::StormLog::new(log_cfg, "t"));
        let sup = Arc::new(Supervisor::new(log, bus));
        let h = sup.clone();
        tokio::spawn(async move { h.run_exit_handler().await });
        (sup, dir)
    }

    /// A server that accepts and never answers: a held mutex.
    async fn stuck_port() -> u16 {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            loop {
                let (s, _) = l.accept().await.unwrap();
                held.push(s);
            }
        });
        port
    }

    fn proc(port: u16, extra: &str) -> crate::config::ProcessConfig {
        toml::from_str(&format!(
            "name = \"engine\"\ncommand = \"/bin/sleep\"\nargs = [\"60\"]\n\
             [[api]]\nname = \"volumes\"\nurl = \"http://127.0.0.1:{port}/api/v1/volumes?limit=1\"\n\
             interval_secs = 1\ntimeout_secs = 1\ninitial_delay_secs = 0\n{extra}"
        ))
        .unwrap()
    }

    /// stormd#49: a stalled API is said and kept, and the process is left
    /// alone unless `restart_after_stalled_secs` says otherwise.
    #[tokio::test]
    async fn a_stall_is_reported_and_not_acted_on_by_default() {
        let (sup, dir) = supervisor("noact").await;
        let port = stuck_port().await;
        sup.start_all(&[proc(port, "")]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(4500)).await;
        let h = sup.api_health().list();
        let s = sup.get_status("engine").await.unwrap();
        sup.stop_all().await;
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].state, ApiState::Stalled);
        assert!(h[0].checks >= 2, "{:?}", h[0]);
        assert_eq!((s.state, s.restarts), (ProcessState::Running, 0), "restarted without being asked");
    }

    #[tokio::test]
    async fn restart_after_stalled_secs_restarts_through_the_policy() {
        let (sup, dir) = supervisor("act").await;
        let port = stuck_port().await;
        sup.start_all(&[proc(port, "restart_after_stalled_secs = 2\n")]).await.unwrap();
        let first = sup.get_status("engine").await.unwrap().pid;
        let mut restarted = None;
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let s = sup.get_status("engine").await.unwrap();
            if s.restarts >= 1 && s.state == ProcessState::Running {
                restarted = Some(s);
                break;
            }
        }
        sup.stop_all().await;
        let _ = std::fs::remove_dir_all(&dir);
        let s = restarted.expect("not restarted within 10 s of a stall");
        assert_ne!(s.pid, first);
        assert_eq!(s.crashes, 1, "the SIGTERMed exit went through the restart policy as a failure");
    }
}

#[cfg(test)]
mod wait_healthy_tests {
    use super::{Supervisor, WaitOutcome};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    async fn supervisor(label: &str) -> (Arc<Supervisor>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("stormd-wait-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg: crate::config::Config = toml::from_str(&format!(
            "[general]\nname = \"t\"\nlog_dir = \"{}\"\n[stormlog.mcast]\ngroup = \"off\"\n",
            dir.display()
        ))
        .unwrap();
        let mut log_cfg = cfg.stormlog.clone();
        log_cfg.file.log_dir = dir.clone();
        let bus = Arc::new(crate::events::EventBus::new(cfg.events.clone(), "t".into()));
        let log = Arc::new(stormlog::StormLog::new(log_cfg, "t"));
        let sup = Arc::new(Supervisor::new(log, bus));
        let h = sup.clone();
        tokio::spawn(async move { h.run_exit_handler().await });
        (sup, dir)
    }

    fn proc(t: &str) -> crate::config::ProcessConfig {
        toml::from_str(&format!("name = \"svc\"\ncommand = \"/bin/sleep\"\nargs = [\"60\"]\n{t}")).unwrap()
    }

    /// stormd#44: the liveness probe has to pass for the *new* run; and the
    /// status says which run it passed for.
    #[tokio::test]
    async fn waits_for_the_new_runs_liveness() {
        let (sup, dir) = supervisor("live").await;
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move { loop { let _ = l.accept().await; } });
        let p = proc(&format!("[liveness]\ntype = \"tcp\"\nport = {port}\ninitial_delay_secs = 1\ninterval_secs = 1\n"));
        sup.start_all(&[p]).await.unwrap();
        let t = Instant::now();
        let out = sup.restart_and_wait("svc", Duration::from_secs(10)).await.unwrap();
        let st = sup.get_status("svc").await.unwrap();
        sup.stop_all().await;
        let _ = std::fs::remove_dir_all(&dir);
        match out {
            WaitOutcome::Healthy { run, .. } => {
                assert_eq!(run, 2);
                assert_eq!(st.liveness_passed_run, Some(2));
                assert!(st.healthy);
            }
            o => panic!("{o:?}"),
        }
        assert!(t.elapsed() >= Duration::from_secs(2), "answered before the new run's first probe");
    }

    #[tokio::test]
    async fn times_out_naming_what_it_waits_on_and_leaves_it_running() {
        let (sup, dir) = supervisor("timeout").await;
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let p = proc(&format!("[liveness]\ntype = \"tcp\"\nport = {closed}\ninitial_delay_secs = 0\ninterval_secs = 1\nfailure_threshold = 100\n"));
        sup.start_all(&[p]).await.unwrap();
        let out = sup.restart_and_wait("svc", Duration::from_secs(2)).await.unwrap();
        let st = sup.get_status("svc").await.unwrap();
        sup.stop_all().await;
        let _ = std::fs::remove_dir_all(&dir);
        match out {
            WaitOutcome::Timeout { waiting_on, .. } => assert!(waiting_on[0].contains("liveness"), "{waiting_on:?}"),
            o => panic!("{o:?}"),
        }
        assert_eq!(st.state, super::ProcessState::Running, "left running for the caller to decide");
    }

    #[tokio::test]
    async fn no_checks_settles_and_an_exit_is_reported() {
        let (sup, dir) = supervisor("settle").await;
        sup.start_all(&[proc("")]).await.unwrap();
        let out = sup.restart_and_wait("svc", Duration::from_secs(10)).await.unwrap();
        assert!(matches!(out, WaitOutcome::Healthy { waited, .. } if waited >= Duration::from_secs(3)), "{out:?}");
        sup.stop_all().await;

        let (sup2, dir2) = supervisor("exit").await;
        let p: crate::config::ProcessConfig = toml::from_str(
            "name = \"svc\"\ncommand = \"/bin/sh\"\nargs = [\"-c\", \"sleep 1; exit 3\"]\nrestart_delay_secs = 5\n",
        )
        .unwrap();
        sup2.start_all(&[p]).await.unwrap();
        let out = sup2.restart_and_wait("svc", Duration::from_secs(10)).await.unwrap();
        sup2.stop_all().await;
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
        assert!(matches!(out, WaitOutcome::Exited { .. }), "{out:?}");
    }

    /// stormd#46: a restarted process with a ready_probe is ready again.
    #[tokio::test]
    async fn a_restart_is_ready_again() {
        let (sup, dir) = supervisor("ready").await;
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move { loop { let _ = l.accept().await; } });
        let p = proc(&format!("ready_probe = {{ type = \"tcp\", port = {port}, interval_secs = 1 }}\n"));
        sup.start_all(&[p]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(sup.get_status("svc").await.unwrap().ready);
        let out = sup.restart_and_wait("svc", Duration::from_secs(10)).await.unwrap();
        let st = sup.get_status("svc").await.unwrap();
        sup.stop_all().await;
        let _ = std::fs::remove_dir_all(&dir);
        assert!(matches!(out, WaitOutcome::Healthy { run: 2, .. }), "{out:?}");
        assert!(st.ready && st.ready_at.is_some(), "not ready after the restart");
    }
}
