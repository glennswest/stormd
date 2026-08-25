use crate::types::{FileConfig, LogEntry};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use tokio::sync::Mutex;
use tracing::{error, info, warn};

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

        // The format lives next to its parser, in `store`, so the two cannot
        // drift into a console that shows every line as INFO at the epoch.
        let line = crate::store::format_line(entry);

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

    /// Take the current log file for a process off the hot path.
    ///
    /// Renames `{process}.log` to a run-specific archive name and resets the
    /// writer so the next write creates a fresh file. Returns the path to the
    /// renamed file (ready for upload to MinIO), or None if there's no file.
    pub async fn take_file(&self, process: &str, run_id: &str, failed: bool) -> Option<PathBuf> {
        let mut writers = self.writers.lock().await;
        writers.remove(process);

        let src = self.config.log_dir.join(format!("{}.log", process));
        if !src.exists() || std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0) == 0 {
            // No file or empty — nothing to archive
            let _ = std::fs::remove_file(&src);
            return None;
        }

        let tag = if failed { "failed" } else { "exited" };
        let dest = self.config.log_dir.join(format!("{}.{}.{}.log", process, run_id, tag));
        match std::fs::rename(&src, &dest) {
            Ok(_) => {
                info!(
                    process = %process, run_id = %run_id, tag = %tag,
                    path = %dest.display(), "log file ready for archive"
                );
                Some(dest)
            }
            Err(e) => {
                warn!(error = %e, "failed to rename log file for archive");
                // Return original path — caller can still try to upload it
                Some(src)
            }
        }
    }

    /// Remove any old rotated files for a process to free disk space.
    /// Called after a successful archive to MinIO.
    pub fn cleanup_rotated(&self, process: &str) {
        let dir = &self.config.log_dir;
        for i in 1..=self.config.max_files {
            let path = dir.join(format!("{}.{}.log", process, i));
            if path.exists() {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    pub fn log_dir(&self) -> &Path {
        &self.config.log_dir
    }

    /// Push every open log file down to the volume.
    ///
    /// Best effort and on demand: doing it per line would make logging a
    /// synchronous write path, which is the other way to make logging the thing
    /// that stops a container.
    pub async fn sync_all(&self) {
        let writers = self.writers.lock().await;
        for w in writers.values() {
            if let Ok(f) = std::fs::OpenOptions::new().append(true).open(&w.path) {
                let _ = f.sync_data();
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
