use crate::types::{FileConfig, LogEntry};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
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
    /// Set while the file cannot be opened or written (stormd#1).
    failing: Option<Failing>,
}

/// A process's log file that cannot be written, and what has been said
/// about it.
///
/// **One ERROR per line flooded the host's log.** Every entry reopened the
/// file, and every failed open logged its own ERROR: two noisy containers
/// with a vanished log directory rotated everyone else's output out of a
/// 1000-entry router log in minutes. Now the failure is said once, retried
/// at most every [`RETRY`], recalled at most every [`REMIND`], and the
/// file says how many lines it is missing when it works again.
#[derive(Debug)]
struct Failing {
    since: Instant,
    next_try: Instant,
    last_said: Instant,
    error: String,
    /// Lines not written to the file while it failed.
    lost: u64,
    #[cfg(test)]
    attempts: u64,
    #[cfg(test)]
    said: u64,
}

/// How often a failing file is tried again.
const RETRY: Duration = Duration::from_secs(1);
/// How often a failure still going on is said again.
const REMIND: Duration = Duration::from_secs(60);

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
        self.write_at(entry, Instant::now()).await
    }

    async fn write_at(&self, entry: &LogEntry, now: Instant) {
        let mut writers = self.writers.lock().await;
        let writer = writers
            .entry(entry.process.clone())
            .or_insert_with(|| {
                let path = self.config.log_dir.join(format!("{}.log", entry.process));
                let current_size = std::fs::metadata(&path)
                    .map(|m| m.len())
                    .unwrap_or(0);
                ProcessWriter { path, current_size, failing: None }
            });

        // Failing, and not yet time to try again: the line is not written to
        // the file (the group and the streams still have it).
        if let Some(f) = &mut writer.failing {
            if now < f.next_try {
                f.lost += 1;
                if now.duration_since(f.last_said) >= REMIND {
                    f.last_said = now;
                    #[cfg(test)]
                    {
                        f.said += 1;
                    }
                    error!(
                        path = %writer.path.display(), error = %f.error, lines_not_written = f.lost,
                        failing_secs = now.duration_since(f.since).as_secs(),
                        "log file still cannot be written"
                    );
                }
                return;
            }
        }

        // Check rotation before writing
        if writer.current_size >= self.config.max_size_bytes {
            self.rotate(&writer.path);
            writer.current_size = 0;
        }

        // The format lives next to its parser, in `store`, so the two cannot
        // drift into a console that shows every line as INFO at the epoch.
        let mut text = String::new();
        if let Some(f) = &writer.failing {
            text.push_str(&crate::store::format_line(&LogEntry::new(
                &entry.process,
                crate::types::LogStream::Stderr,
                format!(
                    "--- stormd: {} line(s) not written to this file over {} s ({}) ---",
                    f.lost,
                    now.duration_since(f.since).as_secs(),
                    f.error
                ),
            )));
        }
        text.push_str(&crate::store::format_line(entry));

        match self.append(&writer.path, text.as_bytes()) {
            Ok(()) => {
                writer.current_size += text.len() as u64;
                if let Some(f) = writer.failing.take() {
                    info!(
                        path = %writer.path.display(), lines_not_written = f.lost,
                        failing_secs = now.duration_since(f.since).as_secs(),
                        "log file writable again"
                    );
                }
            }
            Err(e) => {
                let e = e.to_string();
                match &mut writer.failing {
                    Some(f) => {
                        f.next_try = now + RETRY;
                        f.lost += 1;
                        f.error = e;
                        #[cfg(test)]
                        {
                            f.attempts += 1;
                        }
                    }
                    None => {
                        error!(
                            path = %writer.path.display(), error = %e,
                            "log file cannot be written — retrying every 1 s; lines go to the group and streams meanwhile"
                        );
                        writer.failing = Some(Failing {
                            since: now,
                            next_try: now + RETRY,
                            last_said: now,
                            error: e,
                            lost: 1,
                            #[cfg(test)]
                            attempts: 1,
                            #[cfg(test)]
                            said: 1,
                        });
                    }
                }
            }
        }
    }

    /// (open attempts, ERRORs said, lines lost) while a process's file fails.
    #[cfg(test)]
    async fn failing_counts(&self, process: &str) -> Option<(u64, u64, u64)> {
        let w = self.writers.lock().await;
        let f = w.get(process)?.failing.as_ref()?;
        Some((f.attempts, f.said, f.lost))
    }

    /// Append to a log file. A missing directory — removed, or a mount that
    /// came late — is created once and the open retried.
    fn append(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        let open = || std::fs::OpenOptions::new().create(true).append(true).open(path);
        let mut file = match open() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir_all(path.parent().unwrap_or(&self.config.log_dir))?;
                open()?
            }
            r => r?,
        };
        file.write_all(bytes)
    }

    /// Take the current log file for a process off the hot path.
    ///
    /// Renames `{process}.log` to a run-specific archive name and resets the
    /// writer so the next write creates a fresh file. Returns the path to the
    /// renamed file, or None if there's no file.
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

#[cfg(test)]
mod failure_tests {
    use super::FileLogger;
    use crate::types::{FileConfig, LogEntry, LogStream};
    use std::time::{Duration, Instant};

    fn logger(label: &str) -> (FileLogger, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("stormlog-fail-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&dir);
        let cfg = FileConfig { log_dir: dir.clone(), ..FileConfig::default() };
        let l = FileLogger::new(cfg);
        l.init().unwrap();
        (l, dir)
    }

    fn e(line: &str) -> LogEntry {
        LogEntry::new("app", LogStream::Stdout, line)
    }

    /// stormd#1: the directory removed under a running stormd is made again.
    #[tokio::test]
    async fn a_removed_log_dir_is_recreated() {
        let (l, dir) = logger("gone");
        l.write(&e("one")).await;
        std::fs::remove_dir_all(&dir).unwrap();
        l.write(&e("two")).await;
        let text = std::fs::read_to_string(dir.join("app.log")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(text.contains("two"), "{text}");
    }

    /// A persistent failure is said once, tried at most every second,
    /// recalled at most every minute, and the file says what it missed.
    #[tokio::test]
    async fn a_persistent_failure_backs_off_and_recovery_marks_the_gap() {
        let (l, dir) = logger("stuck");
        // The log directory becomes a plain file: create_dir_all cannot fix that.
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::write(&dir, b"not a directory").unwrap();

        let t0 = Instant::now();
        for i in 0..1000 {
            l.write_at(&e(&format!("line {i}")), t0).await;
        }
        let (attempts, said, lost) = l.failing_counts("app").await.unwrap();
        assert_eq!((attempts, said, lost), (1, 1, 1000), "1000 lines at one instant: one try, one ERROR");

        // A few seconds later: tried again (once per second at most), nothing more said.
        for s in 1..=5u64 {
            l.write_at(&e("later"), t0 + Duration::from_secs(s)).await;
            l.write_at(&e("later"), t0 + Duration::from_secs(s) + Duration::from_millis(10)).await;
        }
        let (attempts, said, _) = l.failing_counts("app").await.unwrap();
        assert_eq!((attempts, said), (6, 1));

        // A minute on: one reminder.
        l.write_at(&e("much later"), t0 + Duration::from_millis(65_500)).await;
        l.write_at(&e("much later"), t0 + Duration::from_millis(65_600)).await;
        assert_eq!(l.failing_counts("app").await.unwrap().1, 2);

        // Fixed: the next try writes, with one marker line for the gap.
        std::fs::remove_file(&dir).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        l.write_at(&e("back"), t0 + Duration::from_secs(70)).await;
        assert!(l.failing_counts("app").await.is_none(), "still failing after recovery");
        let text = std::fs::read_to_string(dir.join("app.log")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(lines[0].contains("line(s) not written to this file"), "{text}");
        assert!(lines[0].contains("1012"), "1000 + 10 + 2 lines lost: {text}");
        assert!(lines[1].contains("back"));
    }
}
