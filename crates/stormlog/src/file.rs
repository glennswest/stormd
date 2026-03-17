use crate::types::{FileConfig, LogEntry};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use tokio::sync::Mutex;
use tracing::{error, info};

/// File-based log writer with per-process files and rotation.
pub struct FileLogger {
    config: FileConfig,
    writers: Mutex<HashMap<String, ProcessWriter>>,
}

struct ProcessWriter {
    path: PathBuf,
    current_size: u64,
}

impl FileLogger {
    pub fn new(config: FileConfig) -> Self {
        Self {
            config,
            writers: Mutex::new(HashMap::new()),
        }
    }

    /// Ensure the log directory exists.
    pub fn init(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.config.log_dir)
    }

    /// Write a log entry to the appropriate process log file.
    pub async fn write(&self, entry: &LogEntry) {
        let mut writers = self.writers.lock().await;
        let writer = writers
            .entry(entry.process.clone())
            .or_insert_with(|| {
                let path = self.config.log_dir.join(format!("{}.log", entry.process));
                let current_size = std::fs::metadata(&path)
                    .map(|m| m.len())
                    .unwrap_or(0);
                ProcessWriter { path, current_size }
            });

        // Check rotation before writing
        if writer.current_size >= self.config.max_size_bytes {
            self.rotate(&writer.path);
            writer.current_size = 0;
        }

        // Format: timestamp [stream] line
        let line = format!(
            "{} [{}] {}\n",
            entry.timestamp.format("%Y-%m-%dT%H:%M:%S%.3fZ"),
            entry.stream,
            entry.line,
        );

        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&writer.path)
        {
            Ok(mut file) => {
                let bytes = line.as_bytes();
                if let Err(e) = file.write_all(bytes) {
                    error!(error = %e, path = %writer.path.display(), "failed to write log");
                } else {
                    writer.current_size += bytes.len() as u64;
                }
            }
            Err(e) => {
                error!(error = %e, path = %writer.path.display(), "failed to open log file");
            }
        }
    }

    /// Rotate log files: .log -> .1.log -> .2.log -> ... -> .N.log (deleted)
    fn rotate(&self, path: &PathBuf) {
        let stem = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let dir = path.parent().unwrap_or(std::path::Path::new("."));

        // Delete the oldest if at max
        let oldest = dir.join(format!("{}.{}.log", stem, self.config.max_files));
        let _ = std::fs::remove_file(&oldest);

        // Shift existing rotated files up by one
        for i in (1..self.config.max_files).rev() {
            let from = dir.join(format!("{}.{}.log", stem, i));
            let to = dir.join(format!("{}.{}.log", stem, i + 1));
            let _ = std::fs::rename(&from, &to);
        }

        // Move current .log to .1.log
        let first = dir.join(format!("{}.1.log", stem));
        let _ = std::fs::rename(path, &first);

        info!(path = %path.display(), "rotated log file");
    }
}
