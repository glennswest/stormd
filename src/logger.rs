use chrono::Utc;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tracing::{error, info};

use crate::config::LogConfig;

#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub process: String,
    pub stream: String, // "stdout" or "stderr"
    pub line: String,
}

pub struct LogManager {
    log_dir: PathBuf,
    config: LogConfig,
    writers: Mutex<HashMap<String, LogWriter>>,
}

struct LogWriter {
    path: PathBuf,
    current_size: u64,
    max_size: u64,
    max_files: u32,
}

impl LogWriter {
    fn new(path: PathBuf, max_size: u64, max_files: u32) -> Self {
        Self {
            path,
            current_size: 0,
            max_size,
            max_files,
        }
    }

    async fn write_line(&mut self, line: &str) -> anyhow::Result<()> {
        if self.current_size >= self.max_size {
            self.rotate().await?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;
        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;
        self.current_size += line.len() as u64 + 1;
        Ok(())
    }

    async fn rotate(&mut self) -> anyhow::Result<()> {
        // Remove oldest file if at limit
        let oldest = format!("{}.{}", self.path.display(), self.max_files);
        let _ = fs::remove_file(&oldest).await;

        // Shift existing files: .9 -> .10, .8 -> .9, etc.
        for i in (1..self.max_files).rev() {
            let from = format!("{}.{}", self.path.display(), i);
            let to = format!("{}.{}", self.path.display(), i + 1);
            let _ = fs::rename(&from, &to).await;
        }

        // Current file becomes .1
        let rotated = format!("{}.1", self.path.display());
        let _ = fs::rename(&self.path, &rotated).await;

        self.current_size = 0;
        info!(path = %self.path.display(), "log rotated");
        Ok(())
    }
}

impl LogManager {
    pub async fn new(log_dir: PathBuf, config: LogConfig) -> anyhow::Result<Self> {
        fs::create_dir_all(&log_dir).await?;
        Ok(Self {
            log_dir,
            config,
            writers: Mutex::new(HashMap::new()),
        })
    }

    async fn get_or_create_writer(&self, process: &str) -> anyhow::Result<()> {
        let mut writers = self.writers.lock().await;
        if !writers.contains_key(process) {
            let path = self.log_dir.join(format!("{}.log", process));
            let current_size = match fs::metadata(&path).await {
                Ok(m) => m.len(),
                Err(_) => 0,
            };
            let mut writer = LogWriter::new(
                path,
                self.config.max_size_bytes,
                self.config.max_files,
            );
            writer.current_size = current_size;
            writers.insert(process.to_string(), writer);
        }
        Ok(())
    }

    pub async fn write_log(&self, process: &str, stream: &str, line: &str) -> anyhow::Result<()> {
        self.get_or_create_writer(process).await?;

        let formatted = if self.config.json_format {
            let entry = LogEntry {
                timestamp: Utc::now().to_rfc3339(),
                process: process.to_string(),
                stream: stream.to_string(),
                line: line.to_string(),
            };
            serde_json::to_string(&entry)?
        } else if self.config.timestamps {
            format!("{} [{}] [{}] {}", Utc::now().to_rfc3339(), process, stream, line)
        } else {
            format!("[{}] [{}] {}", process, stream, line)
        };

        let mut writers = self.writers.lock().await;
        if let Some(writer) = writers.get_mut(process) {
            writer.write_line(&formatted).await?;
        }
        Ok(())
    }

    /// Capture stdout/stderr from a child process into log files.
    pub fn spawn_capture(
        self: &std::sync::Arc<Self>,
        process_name: String,
        stdout: Option<tokio::process::ChildStdout>,
        stderr: Option<tokio::process::ChildStderr>,
    ) {
        if let Some(stdout) = stdout {
            let mgr = self.clone();
            let name = process_name.clone();
            tokio::spawn(async move {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Err(e) = mgr.write_log(&name, "stdout", &line).await {
                        error!(error = %e, process = %name, "failed to write stdout log");
                    }
                }
            });
        }

        if let Some(stderr) = stderr {
            let mgr = self.clone();
            let name = process_name;
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Err(e) = mgr.write_log(&name, "stderr", &line).await {
                        error!(error = %e, process = %name, "failed to write stderr log");
                    }
                }
            });
        }
    }

    /// Read log lines with optional filters.
    pub async fn read_logs(
        &self,
        process: Option<&str>,
        tail: Option<usize>,
        search: Option<&str>,
    ) -> anyhow::Result<Vec<String>> {
        let mut all_lines = Vec::new();

        let entries = fs::read_dir(&self.log_dir).await;
        let mut entries = match entries {
            Ok(e) => e,
            Err(_) => return Ok(all_lines),
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let file_name = path.file_name().unwrap_or_default().to_string_lossy();

            // Only .log files (not rotated ones for now — just current)
            if !file_name.ends_with(".log") {
                continue;
            }

            if let Some(proc_filter) = process {
                if !file_name.starts_with(proc_filter) {
                    continue;
                }
            }

            if let Ok(content) = fs::read_to_string(&path).await {
                for line in content.lines() {
                    if let Some(pattern) = search {
                        if !line.contains(pattern) {
                            continue;
                        }
                    }
                    all_lines.push(line.to_string());
                }
            }
        }

        // Apply tail
        if let Some(n) = tail {
            let start = all_lines.len().saturating_sub(n);
            all_lines = all_lines[start..].to_vec();
        }

        Ok(all_lines)
    }

    /// List available log files with sizes.
    pub async fn list_files(&self) -> anyhow::Result<Vec<LogFileInfo>> {
        let mut files = Vec::new();
        let mut entries = fs::read_dir(&self.log_dir).await?;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if let Ok(meta) = fs::metadata(&path).await {
                files.push(LogFileInfo {
                    name: path.file_name().unwrap_or_default().to_string_lossy().to_string(),
                    path: path.to_string_lossy().to_string(),
                    size_bytes: meta.len(),
                });
            }
        }
        files.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(files)
    }

    pub fn log_dir(&self) -> &Path {
        &self.log_dir
    }
}

#[derive(Debug, Serialize)]
pub struct LogFileInfo {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
}
