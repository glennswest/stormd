pub mod file;
pub mod ingest;
pub mod storage;
pub mod stream;
pub mod syslog;
pub mod terminal;
pub mod types;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::file::FileLogger;
use crate::storage::LogStorage;
use crate::stream::StreamManager;
use crate::syslog::SyslogReceiver;
use crate::terminal::TerminalManager;
use crate::types::{LogEntry, LogQuery, LogStream, ScreenSnapshot, Severity, StormLogConfig};

/// StormLog — unified logging facade.
///
/// Handles VT100 terminal emulation, broadcast streams, MinIO storage,
/// and syslog reception.
pub struct StormLog {
    config: StormLogConfig,
    container_name: String,
    terminal_manager: TerminalManager,
    stream_manager: StreamManager,
    storage: Arc<LogStorage>,
    file_logger: FileLogger,
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
        let storage = Arc::new(LogStorage::new(config.minio.clone()));
        let file_logger = FileLogger::new(config.file.clone());
        let (ingest_tx, ingest_rx) = mpsc::channel(1024);

        Self {
            config,
            container_name: container_name.into(),
            terminal_manager,
            stream_manager,
            storage,
            file_logger,
            ingest_tx,
            ingest_rx: Mutex::new(Some(ingest_rx)),
            run_ids: Mutex::new(HashMap::new()),
            reader_tasks: Mutex::new(HashMap::new()),
        }
    }

    /// Start all subsystems: file logger, syslog listeners, storage flush loop, ingest receiver.
    pub async fn start(self: &Arc<Self>) {
        // Initialize file logger (local disk)
        if let Err(e) = self.file_logger.init() {
            error!(error = %e, "failed to initialize file logger");
        }

        // Initialize MinIO storage on the actual storage instance
        if self.config.minio.enabled {
            if let Err(e) = self.storage.init().await {
                error!(error = %e, "failed to initialize MinIO storage");
            }
        }

        // Start syslog receivers
        let syslog_tx = self.ingest_tx.clone();
        let syslog = SyslogReceiver::new(self.config.syslog.clone(), syslog_tx);
        syslog.start().await;

        // Start storage flush loop
        if self.config.minio.enabled {
            let storage = self.storage.clone();
            tokio::spawn(async move {
                storage.run_flush_loop().await;
            });
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
                                    let entry = LogEntry::new(&name, LogStream::Stdout, line)
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
                                    let entry = LogEntry::new(&name, LogStream::Stderr, line)
                                        .with_severity(Severity::Warning)
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

    /// Write a log entry to all outputs: file, broadcast stream, storage.
    pub async fn write_entry(&self, entry: LogEntry) {
        // Write to local file
        self.file_logger.write(&entry).await;

        // Publish to broadcast streams
        self.stream_manager.publish(entry.clone()).await;

        // Buffer for MinIO storage
        if self.storage.is_enabled() {
            self.storage.buffer_entry(entry).await;
        }
    }

    /// Archive a process run's logs to MinIO and clean up local disk.
    ///
    /// Called by the supervisor when a process exits. Flushes buffered entries,
    /// takes the local log file, uploads it to MinIO, and removes local files.
    pub async fn archive_run(&self, process: &str, failed: bool) {
        // Get the run_id for this process
        let run_id = {
            let ids = self.run_ids.lock().await;
            ids.get(process).cloned()
        };
        let run_id = match run_id {
            Some(id) => id,
            None => {
                info!(process = %process, "no run_id to archive");
                return;
            }
        };

        // Wait for stdout/stderr reader tasks to finish draining pipe buffers.
        // This ensures all output (especially final stderr on crash) is captured.
        let handles = {
            let mut tasks = self.reader_tasks.lock().await;
            tasks.remove(process).unwrap_or_default()
        };
        if !handles.is_empty() {
            info!(process = %process, tasks = handles.len(), "waiting for reader tasks to drain");
            for h in handles {
                if let Err(e) = tokio::time::timeout(
                    tokio::time::Duration::from_secs(5),
                    h,
                ).await {
                    warn!(process = %process, error = %e, "reader task drain timed out");
                }
            }
        }

        // Flush any buffered MinIO entries first
        if self.storage.is_enabled() {
            if let Err(e) = self.storage.flush().await {
                error!(error = %e, "failed to flush buffer before archive");
            }
        }

        // Take the local log file (renames it out of the hot path)
        let file_path = self.file_logger.take_file(process, &run_id, failed).await;

        // Upload to MinIO if enabled and there's a file
        if self.storage.is_enabled() {
            if let Some(ref path) = file_path {
                match self.storage.archive_file(process, &run_id, failed, path).await {
                    Ok(_) => {
                        // Clean up any rotated files too
                        self.file_logger.cleanup_rotated(process);
                        info!(
                            process = %process, run_id = %run_id,
                            failed = failed, "run archived to MinIO"
                        );
                    }
                    Err(e) => {
                        error!(
                            error = %e, process = %process,
                            "failed to archive to MinIO — local file preserved"
                        );
                    }
                }
            }
        } else if let Some(ref path) = file_path {
            // MinIO not enabled — keep the local archive file but log it
            info!(
                process = %process, run_id = %run_id,
                path = %path.display(),
                "MinIO not enabled — archived log kept locally"
            );
        }
    }

    /// Query logs from MinIO storage.
    pub async fn query_logs(&self, query: &LogQuery) -> anyhow::Result<Vec<LogEntry>> {
        self.storage.query(query).await
    }

    /// List all runs for a process.
    pub async fn list_runs(&self, process: &str) -> anyhow::Result<Vec<storage::RunInfo>> {
        self.storage.list_runs(process).await
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

    /// Flush all buffered entries to storage.
    pub async fn flush(&self) -> anyhow::Result<usize> {
        self.storage.flush().await
    }
}
