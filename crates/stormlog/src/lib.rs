pub mod ingest;
pub mod storage;
pub mod stream;
pub mod syslog;
pub mod terminal;
pub mod types;

use std::sync::Arc;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::sync::mpsc;
use tracing::{error, info};

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
    ingest_tx: mpsc::Sender<LogEntry>,
    ingest_rx: tokio::sync::Mutex<Option<mpsc::Receiver<LogEntry>>>,
}

impl StormLog {
    /// Create a new StormLog instance.
    pub fn new(config: StormLogConfig, container_name: impl Into<String>) -> Self {
        let terminal_manager = TerminalManager::new(config.terminal.rows, config.terminal.cols);
        let stream_manager = StreamManager::new();
        let storage = Arc::new(LogStorage::new(config.minio.clone()));
        let (ingest_tx, ingest_rx) = mpsc::channel(1024);

        Self {
            config,
            container_name: container_name.into(),
            terminal_manager,
            stream_manager,
            storage,
            ingest_tx,
            ingest_rx: tokio::sync::Mutex::new(Some(ingest_rx)),
        }
    }

    /// Start all subsystems: syslog listeners, storage flush loop, ingest receiver.
    pub async fn start(self: &Arc<Self>) {
        // Initialize MinIO storage
        if self.config.minio.enabled {
            let mut storage = LogStorage::new(self.config.minio.clone());
            if let Err(e) = storage.init().await {
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

    /// Capture stdout/stderr from a child process.
    ///
    /// Raw bytes flow through VT100 terminal emulation, then get split into
    /// lines and published to broadcast streams + storage.
    pub fn spawn_capture(
        self: &Arc<Self>,
        process: String,
        stdout: Option<tokio::process::ChildStdout>,
        stderr: Option<tokio::process::ChildStderr>,
    ) {
        if let Some(stdout) = stdout {
            let this = self.clone();
            let name = process.clone();
            tokio::spawn(async move {
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
                                    let entry = LogEntry::new(&name, LogStream::Stdout, line);
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
            });
        }

        if let Some(stderr) = stderr {
            let this = self.clone();
            let name = process;
            tokio::spawn(async move {
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
                                        .with_severity(Severity::Warning);
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
        }
    }

    /// Write a log entry to all outputs: broadcast stream + storage.
    pub async fn write_entry(&self, entry: LogEntry) {
        // Publish to broadcast streams
        self.stream_manager.publish(entry.clone()).await;

        // Buffer for MinIO storage
        if self.storage.is_enabled() {
            self.storage.buffer_entry(entry).await;
        }
    }

    /// Query logs from MinIO storage.
    pub async fn query_logs(&self, query: &LogQuery) -> anyhow::Result<Vec<LogEntry>> {
        self.storage.query(query).await
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
