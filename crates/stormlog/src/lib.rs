//! A container's logging: write it down, put it on the wire, let someone
//! follow it.
//!
//! Three things and no more. Lines go to a file on the log volume, which is
//! rotated and is what `must-gather` collects; they go onto the fleet's
//! multicast group, which is how anyone sees them without being on this node;
//! and they go to a broadcast channel, which is what the web console follows.
//!
//! What used to be here as well — an object store with a bucket and
//! credentials, a flush loop, and syslog receivers on UDP, TCP and `/dev/log`
//! — has gone. Receiving, storing, indexing and searching a fleet's logs is a
//! collector's job (`mcastsyslog`), and doing it in a container's init as well
//! meant two stores, two schemas, and a view that saw half the nodes. Worse,
//! it made the logs a node keeps depend on a service elsewhere being up — and
//! the logs anyone wants are from the failure that also took the network out.

pub mod file;
pub mod mcast;
pub mod store;
pub mod stream;
pub mod terminal;
pub mod types;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::file::FileLogger;
use crate::store::LogStore;
use crate::stream::StreamManager;
use crate::terminal::TerminalManager;
use crate::types::{LogEntry, LogQuery, LogStream, ScreenSnapshot, Severity, StormLogConfig};

/// What a line looks like it is.
///
/// One judgement, in [`stormcast`], shared with the wire and with `stormpump`
/// — so a line has the same severity in the file, on the group and in the
/// console rather than one per program that looked at it. `base` is what the
/// stream implies when the text says nothing: a bare line on stderr is a
/// warning, the same line on stdout is not.
fn detect_severity(line: &str, base: Severity) -> Severity {
    let read = crate::store::severity_of(line);
    if read == Severity::Info {
        base
    } else {
        read
    }
}

/// StormLog — unified logging facade.
///
/// VT100 terminal emulation for the console, broadcast streams for following,
/// a rotated file per process, and the fleet's multicast group.
pub struct StormLog {
    config: StormLogConfig,
    container_name: String,
    terminal_manager: TerminalManager,
    stream_manager: StreamManager,
    /// The log directory, read back. There is no other store.
    store: LogStore,
    file_logger: FileLogger,
    /// The fleet's multicast group, when one is configured. Emitting only —
    /// receiving and storing belongs to a collector, not to a container init.
    mcast: Option<mcast::Emitter>,
    ingest_tx: mpsc::Sender<LogEntry>,
    ingest_rx: Mutex<Option<mpsc::Receiver<LogEntry>>>,
    /// Active run IDs per process — set when spawn_capture is called.
    run_ids: Mutex<HashMap<String, String>>,
    /// Reader task handles per process — awaited before archiving to ensure all output is captured.
    reader_tasks: Mutex<HashMap<String, Vec<JoinHandle<()>>>>,
}

impl StormLog {
    /// Create a new StormLog instance.
    pub fn new(config: StormLogConfig, container_name: impl Into<String>) -> Self {
        let terminal_manager = TerminalManager::new(config.terminal.rows, config.terminal.cols);
        let stream_manager = StreamManager::new();
        let store = LogStore::new(config.file.log_dir.clone());
        let file_logger = FileLogger::new(config.file.clone());
        let (ingest_tx, ingest_rx) = mpsc::channel(1024);

        // The fleet group, unless this container was told otherwise. On by
        // default: a node that has to be configured before anyone can see its
        // logs is a node whose first failure is invisible.
        let name = container_name.into();
        let mcast = config
            .mcast
            .group
            .as_deref()
            .filter(|g| !g.is_empty() && *g != "off")
            .unwrap_or(mcast::DEFAULT_GROUP)
            .parse()
            .ok()
            .and_then(|addr| mcast::Emitter::new(addr, name.clone()));

        Self {
            config,
            container_name: name,
            terminal_manager,
            stream_manager,
            store,
            file_logger,
            mcast,
            ingest_tx,
            ingest_rx: Mutex::new(Some(ingest_rx)),
            run_ids: Mutex::new(HashMap::new()),
            reader_tasks: Mutex::new(HashMap::new()),
        }
    }

    /// Start the file logger and the ingest receiver. Nothing here binds a
    /// port, opens a connection, or waits on anything.
    pub async fn start(self: &Arc<Self>) {
        if let Err(e) = self.file_logger.init() {
            error!(error = %e, "failed to initialize file logger");
        }

        // Start ingest receiver loop
        let this = self.clone();
        let rx = self.ingest_rx.lock().await.take();
        if let Some(mut rx) = rx {
            tokio::spawn(async move {
                while let Some(entry) = rx.recv().await {
                    this.write_entry(entry).await;
                }
            });
        }

        info!(container = %self.container_name, "stormlog started");
    }

    /// Generate a run ID for a process start. Called each time a process spawns.
    fn make_run_id() -> String {
        chrono::Utc::now().format("%Y%m%dT%H%M%S").to_string()
    }

    /// Capture stdout/stderr from a child process.
    ///
    /// Each call creates a new run — when the process restarts, logs are
    /// stored under a new run_id so you can distinguish between runs.
    pub async fn spawn_capture(
        self: &Arc<Self>,
        process: String,
        stdout: Option<tokio::process::ChildStdout>,
        stderr: Option<tokio::process::ChildStderr>,
    ) {
        let run_id = Self::make_run_id();
        info!(process = %process, run_id = %run_id, "starting log capture");

        // Store the run_id for this process
        {
            let mut ids = self.run_ids.lock().await;
            ids.insert(process.clone(), run_id.clone());
        }

        // Emit a marker entry for the run start
        let marker = LogEntry::new(&process, LogStream::Stdout, "--- process started ---")
            .with_run_id(&run_id);
        self.write_entry(marker).await;

        let mut handles: Vec<JoinHandle<()>> = Vec::new();

        if let Some(stdout) = stdout {
            let this = self.clone();
            let name = process.clone();
            let rid = run_id.clone();
            let h = tokio::spawn(async move {
                let mut reader = BufReader::new(stdout);
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            let bytes = &buf[..n];
                            // Feed raw bytes to VT100
                            this.terminal_manager.feed(&name, bytes).await;
                            // Split into lines and publish
                            let text = String::from_utf8_lossy(bytes);
                            for line in text.lines() {
                                if !line.is_empty() {
                                    let sev = detect_severity(line, Severity::Info);
                                    let entry = LogEntry::new(&name, LogStream::Stdout, line)
                                        .with_severity(sev)
                                        .with_run_id(&rid);
                                    this.write_entry(entry).await;
                                }
                            }
                        }
                        Err(e) => {
                            error!(error = %e, process = %name, "stdout read error");
                            break;
                        }
                    }
                }
                // Emit end marker
                let end = LogEntry::new(&name, LogStream::Stdout, "--- process exited ---")
                    .with_run_id(&rid);
                this.write_entry(end).await;
            });
            handles.push(h);
        }

        if let Some(stderr) = stderr {
            let this = self.clone();
            let name = process.clone();
            let rid = run_id.clone();
            let h = tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            let bytes = &buf[..n];
                            // Stderr also goes through VT100
                            this.terminal_manager.feed(&name, bytes).await;
                            let text = String::from_utf8_lossy(bytes);
                            for line in text.lines() {
                                if !line.is_empty() {
                                    let sev = detect_severity(line, Severity::Warning);
                                    let entry = LogEntry::new(&name, LogStream::Stderr, line)
                                        .with_severity(sev)
                                        .with_run_id(&rid);
                                    this.write_entry(entry).await;
                                }
                            }
                        }
                        Err(e) => {
                            error!(error = %e, process = %name, "stderr read error");
                            break;
                        }
                    }
                }
            });
            handles.push(h);
        }

        // Store handles so archive_run can await them
        {
            let mut tasks = self.reader_tasks.lock().await;
            tasks.insert(process, handles);
        }
    }

    /// Write an entry to all three: the file, the group, and whoever is
    /// following.
    pub async fn write_entry(&self, entry: LogEntry) {
        // Write to local file
        self.file_logger.write(&entry).await;

        // Put it on the fleet's multicast group.
        //
        // Here, because this is the one funnel every line passes through, and
        // because a datagram cannot block: the local file is the record and
        // the group is how anyone sees it without being on this node.
        if let Some(m) = &self.mcast {
            m.send(&entry);
        }

        // Publish to broadcast streams — this is what a console follows.
        self.stream_manager.publish(entry).await;
    }

    /// Emit a process crash/failure entry at Emergency severity.
    pub async fn emit_crash(&self, process: &str, exit_code: Option<i32>) {
        let run_id = {
            let ids = self.run_ids.lock().await;
            ids.get(process).cloned()
        };
        let msg = match exit_code {
            Some(code) => format!("*** PROCESS CRASHED *** exit code {}", code),
            None => "*** PROCESS CRASHED *** killed by signal".to_string(),
        };
        let mut entry = LogEntry::new(process, LogStream::Stderr, msg)
            .with_severity(Severity::Emergency);
        if let Some(rid) = run_id {
            entry = entry.with_run_id(rid);
        }
        self.write_entry(entry).await;
    }

    /// Close out a process run: name its log file after the run and prune old
    /// ones.
    ///
    /// Called by the supervisor when a process exits. The file stays exactly
    /// where it was — this only renames it out of the hot path so the next run
    /// starts a fresh one, and the console can still list and read it.
    pub async fn archive_run(&self, process: &str, failed: bool) {
        let run_id = {
            let ids = self.run_ids.lock().await;
            ids.get(process).cloned()
        };
        let Some(run_id) = run_id else {
            info!(process = %process, "no run_id to archive");
            return;
        };

        // Wait for the stdout/stderr readers to drain their pipes first. On a
        // crash the last thing written is the reason for it, and it is still
        // in a pipe buffer when the process is already gone.
        let handles = {
            let mut tasks = self.reader_tasks.lock().await;
            tasks.remove(process).unwrap_or_default()
        };
        if !handles.is_empty() {
            info!(process = %process, tasks = handles.len(), "waiting for reader tasks to drain");
            for h in handles {
                if let Err(e) =
                    tokio::time::timeout(tokio::time::Duration::from_secs(5), h).await
                {
                    warn!(process = %process, error = %e, "reader task drain timed out");
                }
            }
        }

        let file_path = self.file_logger.take_file(process, &run_id, failed).await;

        // The volume has a size, and a process that restarts in a loop writes
        // one archive per restart. Without this the thing that fills the log
        // volume is the record of what went wrong.
        let pruned = self.store.prune(process, self.config.file.max_runs);
        if pruned > 0 {
            info!(process = %process, pruned, "pruned old runs");
        }

        if let Some(path) = file_path {
            info!(
                process = %process, run_id = %run_id, failed = failed,
                path = %path.display(), "run closed"
            );
        }
    }

    /// Read back what is on the log volume.
    pub async fn query_logs(&self, query: &LogQuery) -> anyhow::Result<Vec<LogEntry>> {
        Ok(self.store.query(query))
    }

    /// The runs of a process, newest first, the live one ahead of them.
    pub async fn list_runs(&self, process: &str) -> anyhow::Result<Vec<store::RunInfo>> {
        Ok(self.store.runs(process))
    }

    /// Get the current run_id for a process (if capturing).
    pub async fn current_run_id(&self, process: &str) -> Option<String> {
        let ids = self.run_ids.lock().await;
        ids.get(process).cloned()
    }

    /// Get a VT100 screen snapshot for a process.
    pub async fn get_screen(&self, process: &str) -> Option<ScreenSnapshot> {
        self.terminal_manager.snapshot(process).await
    }

    /// Subscribe to a specific process's log stream.
    pub async fn subscribe_process(
        &self,
        process: &str,
    ) -> tokio::sync::broadcast::Receiver<LogEntry> {
        self.stream_manager.subscribe_process(process).await
    }

    /// Subscribe to all log entries across all processes.
    pub fn subscribe_all(&self) -> tokio::sync::broadcast::Receiver<LogEntry> {
        self.stream_manager.subscribe_all()
    }

    /// Get a sender for external log ingest (REST API, etc).
    pub fn ingest_sender(&self) -> mpsc::Sender<LogEntry> {
        self.ingest_tx.clone()
    }

    /// Get terminal manager reference for WebSocket/SSH access.
    pub fn terminal_manager(&self) -> &TerminalManager {
        &self.terminal_manager
    }

    /// Push what has been written down to the volume.
    ///
    /// Nothing is buffered in this process — every line is written when it
    /// arrives — so this is an `fsync` and not a drain. It matters anyway: a
    /// line in the page cache is lost by the panic it was describing.
    pub async fn flush(&self) -> anyhow::Result<usize> {
        self.file_logger.sync_all().await;
        Ok(0)
    }
}
