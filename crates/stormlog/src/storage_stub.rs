//! No-op log storage — compiled when the `s3` feature is disabled.
//!
//! Local file logging (`FileLogger`) and live streaming still work; only the
//! MinIO/S3 archival + historical query backend is compiled out. This keeps the
//! `rust-s3` dependency (and its second HTTP client) off memory-limited targets
//! such as the armv7 travel build.

use crate::types::{LogEntry, LogQuery, MinioConfig};

/// Drop-in replacement for the S3-backed `LogStorage` with an identical public
/// API. Every operation is a no-op / empty result.
pub struct LogStorage {
    _config: MinioConfig,
}

impl LogStorage {
    pub fn new(config: MinioConfig) -> Self {
        Self { _config: config }
    }

    pub async fn init(&self) -> anyhow::Result<()> {
        Ok(())
    }

    pub async fn buffer_entry(&self, _entry: LogEntry) {}

    pub async fn flush(&self) -> anyhow::Result<usize> {
        Ok(0)
    }

    pub async fn query(&self, _query: &LogQuery) -> anyhow::Result<Vec<LogEntry>> {
        Ok(Vec::new())
    }

    pub async fn list_runs(&self, _process: &str) -> anyhow::Result<Vec<RunInfo>> {
        Ok(Vec::new())
    }

    pub async fn archive_file(
        &self,
        _process: &str,
        _run_id: &str,
        _failed: bool,
        _file_path: &std::path::Path,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    pub async fn run_flush_loop(&self) {}

    pub fn is_enabled(&self) -> bool {
        false
    }
}

/// Metadata about a process run (mirrors the S3 backend's type).
#[derive(Debug, Clone, serde::Serialize)]
pub struct RunInfo {
    pub run_id: String,
    pub process: String,
    pub date: String,
    pub size_bytes: u64,
    pub object_count: u32,
}
