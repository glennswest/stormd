use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Canonical log unit — every log line becomes one of these.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub process: String,
    pub stream: LogStream,
    pub line: String,
    pub severity: Severity,
    /// Run identifier — groups log entries by process lifecycle.
    /// Each time a process starts (or restarts), a new run_id is assigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

impl LogEntry {
    pub fn new(process: impl Into<String>, stream: LogStream, line: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            process: process.into(),
            stream,
            line: line.into(),
            severity: Severity::Info,
            run_id: None,
        }
    }

    pub fn with_severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    pub fn with_run_id(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = Some(run_id.into());
        self
    }
}

/// Where the log line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    Stdout,
    Stderr,
    Syslog,
    Ingest,
}

impl std::fmt::Display for LogStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stdout => write!(f, "stdout"),
            Self::Stderr => write!(f, "stderr"),
            Self::Syslog => write!(f, "syslog"),
            Self::Ingest => write!(f, "ingest"),
        }
    }
}

/// Syslog severity levels (RFC 5424).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Emergency = 0,
    Alert = 1,
    Critical = 2,
    Error = 3,
    Warning = 4,
    Notice = 5,
    Info = 6,
    Debug = 7,
}

impl Severity {
    pub fn from_syslog_priority(priority: u8) -> Self {
        match priority & 0x07 {
            0 => Self::Emergency,
            1 => Self::Alert,
            2 => Self::Critical,
            3 => Self::Error,
            4 => Self::Warning,
            5 => Self::Notice,
            6 => Self::Info,
            _ => Self::Debug,
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Emergency => write!(f, "emerg"),
            Self::Alert => write!(f, "alert"),
            Self::Critical => write!(f, "crit"),
            Self::Error => write!(f, "error"),
            Self::Warning => write!(f, "warn"),
            Self::Notice => write!(f, "notice"),
            Self::Info => write!(f, "info"),
            Self::Debug => write!(f, "debug"),
        }
    }
}

/// Query parameters for log retrieval.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LogQuery {
    pub process: Option<String>,
    pub stream: Option<LogStream>,
    pub severity_min: Option<Severity>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub search: Option<String>,
    pub tail: Option<usize>,
    /// Filter to a specific run. If None, returns logs from all runs.
    pub run_id: Option<String>,
}

/// Snapshot of a process's VT100 terminal screen.
#[derive(Debug, Clone, Serialize)]
pub struct ScreenSnapshot {
    pub process: String,
    pub rows: u16,
    pub cols: u16,
    pub contents: String,
    pub cursor_row: u16,
    pub cursor_col: u16,
}

/// File-based log storage configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct FileConfig {
    /// Directory to write log files. Each process gets `{process}.log`.
    #[serde(default = "default_log_dir")]
    pub log_dir: std::path::PathBuf,
    /// Maximum size of a single log file before rotation.
    #[serde(default = "default_max_size_bytes")]
    pub max_size_bytes: u64,
    /// Maximum number of rotated files to keep per process.
    #[serde(default = "default_max_files")]
    pub max_files: u32,
}

impl Default for FileConfig {
    fn default() -> Self {
        Self {
            log_dir: default_log_dir(),
            max_size_bytes: default_max_size_bytes(),
            max_files: default_max_files(),
        }
    }
}

fn default_log_dir() -> std::path::PathBuf {
    std::path::PathBuf::from("/var/log/stormd")
}
fn default_max_size_bytes() -> u64 {
    100 * 1024 * 1024 // 100 MiB
}
fn default_max_files() -> u32 {
    10
}

/// Configuration for StormLog.
#[derive(Debug, Clone, Deserialize)]
pub struct StormLogConfig {
    #[serde(default)]
    pub file: FileConfig,
    #[serde(default)]
    pub minio: MinioConfig,
    #[serde(default)]
    pub syslog: SyslogConfig,
    #[serde(default)]
    pub terminal: TerminalConfig,
    #[serde(default)]
    pub mcast: McastConfig,
}

/// Where this container's lines go on the network.
///
/// Emitting only. A container init has no business holding a log database —
/// receiving, storing, indexing and searching is a collector's job, and doing
/// it in both places means two stores, two schemas, and a fleet view that sees
/// half the nodes.
#[derive(Debug, Clone, Deserialize)]
pub struct McastConfig {
    /// `host:port`, or `off`. Defaults to the fleet group, because a node that
    /// must be configured before anyone can see its logs is a node whose first
    /// failure is invisible.
    #[serde(default)]
    pub group: Option<String>,
}

impl Default for McastConfig {
    fn default() -> Self {
        Self { group: None }
    }
}

impl Default for StormLogConfig {
    fn default() -> Self {
        Self {
            file: FileConfig::default(),
            minio: MinioConfig::default(),
            syslog: SyslogConfig::default(),
            terminal: TerminalConfig::default(),
            mcast: McastConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MinioConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_minio_endpoint")]
    pub endpoint: String,
    #[serde(default = "default_minio_bucket")]
    pub bucket: String,
    #[serde(default = "default_minio_access_key")]
    pub access_key: String,
    #[serde(default = "default_minio_secret_key")]
    pub secret_key: String,
    #[serde(default = "default_minio_region")]
    pub region: String,
}

impl Default for MinioConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: default_minio_endpoint(),
            bucket: default_minio_bucket(),
            access_key: default_minio_access_key(),
            secret_key: default_minio_secret_key(),
            region: default_minio_region(),
        }
    }
}

fn default_minio_endpoint() -> String { "http://127.0.0.1:9000".to_string() }
fn default_minio_bucket() -> String { "logs".to_string() }
fn default_minio_access_key() -> String { "stormd".to_string() }
fn default_minio_secret_key() -> String { "stormdpass".to_string() }
fn default_minio_region() -> String { "us-east-1".to_string() }

#[derive(Debug, Clone, Deserialize)]
pub struct SyslogConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_syslog_udp_bind")]
    pub udp_bind: String,
    #[serde(default = "default_syslog_tcp_bind")]
    pub tcp_bind: String,
    #[serde(default = "default_unix_socket")]
    pub unix_socket: String,
}

impl Default for SyslogConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            udp_bind: default_syslog_udp_bind(),
            tcp_bind: default_syslog_tcp_bind(),
            unix_socket: default_unix_socket(),
        }
    }
}

fn default_syslog_udp_bind() -> String { "127.0.0.1:514".to_string() }
fn default_syslog_tcp_bind() -> String { "127.0.0.1:514".to_string() }
fn default_unix_socket() -> String { "/dev/log".to_string() }

#[derive(Debug, Clone, Deserialize)]
pub struct TerminalConfig {
    #[serde(default = "default_rows")]
    pub rows: u16,
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default = "default_scrollback")]
    pub scrollback: usize,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            rows: default_rows(),
            cols: default_cols(),
            scrollback: default_scrollback(),
        }
    }
}

fn default_rows() -> u16 { 24 }
fn default_cols() -> u16 { 80 }
fn default_scrollback() -> usize { 1000 }

/// Request body for REST log ingest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestRequest {
    pub process: String,
    #[serde(default = "default_ingest_stream")]
    pub stream: LogStream,
    pub line: String,
    pub severity: Option<Severity>,
}

fn default_ingest_stream() -> LogStream { LogStream::Ingest }
