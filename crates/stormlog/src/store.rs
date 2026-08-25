//! Reading back what was written to disk.
//!
//! There was an object store behind this once — MinIO, a bucket, a flush loop,
//! a buffer, credentials — and every run was uploaded to it when a process
//! exited. That is a lot of machinery for a container's init to carry, and it
//! made the logs a node keeps depend on a service somewhere else being up,
//! which is exactly backwards: the logs anyone wants are the ones from the
//! failure that also took the network out.
//!
//! So the file *is* the store. [`crate::file::FileLogger`] writes a line per
//! entry and rotates at a size; this module reads those same files back and
//! answers a [`LogQuery`] from them. The directory is a volume — the log PVC —
//! so what is on it survives the container, and `must-gather` collects it by
//! copying a directory rather than by asking anything.
//!
//! Live tailing does not come through here at all: a follower subscribes to the
//! broadcast stream and sees lines as they are written. This is for what
//! already happened.

use crate::types::{LogEntry, LogQuery, LogStream, Severity};
use chrono::{DateTime, TimeZone, Utc};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// One line on disk.
///
/// Written and parsed side by side on purpose. A format with its writer in one
/// file and its reader in another drifts, and the drift shows up as a console
/// that displays every line as INFO with the epoch for a timestamp.
///
/// ```text
/// 2026-08-25T11:50:09.123Z stderr error something broke
/// ```
///
/// Four fields and then the rest of the line, so a message containing spaces —
/// which is every message — needs no quoting or escaping.
pub fn format_line(entry: &LogEntry) -> String {
    format!(
        "{} {} {} {}\n",
        entry.timestamp.format("%Y-%m-%dT%H:%M:%S%.3fZ"),
        entry.stream,
        entry.severity,
        entry.line,
    )
}

/// Parse one line back.
///
/// A line that does not parse is still returned, whole, rather than dropped: an
/// unreadable log line is evidence too, and the one thing a log reader must
/// never do is decide something did not happen because it could not read it.
fn parse_line(process: &str, raw: &str) -> LogEntry {
    let mut parts = raw.splitn(4, ' ');
    let (ts, stream, sev, msg) = (parts.next(), parts.next(), parts.next(), parts.next());

    let parsed = ts
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.with_timezone(&Utc));

    match (parsed, stream, sev, msg) {
        (Some(timestamp), Some(stream), Some(sev), Some(msg)) => {
            let mut e = LogEntry::new(process, parse_stream(stream), msg);
            e.timestamp = timestamp;
            e.severity = parse_severity(sev);
            e
        }
        _ => {
            // Something else wrote here, or the format changed under us. Guess
            // the severity from the text and keep the line as it stands.
            let sev = severity_of(raw);
            let mut e = LogEntry::new(process, LogStream::Stdout, raw);
            e.severity = sev;
            e.timestamp = Utc.timestamp_opt(0, 0).single().unwrap_or_else(Utc::now);
            e
        }
    }
}

fn parse_stream(s: &str) -> LogStream {
    match s {
        "stderr" => LogStream::Stderr,
        "syslog" => LogStream::Syslog,
        "ingest" => LogStream::Ingest,
        _ => LogStream::Stdout,
    }
}

fn parse_severity(s: &str) -> Severity {
    match s {
        "emerg" => Severity::Emergency,
        "alert" => Severity::Alert,
        "crit" => Severity::Critical,
        "error" => Severity::Error,
        "warn" => Severity::Warning,
        "notice" => Severity::Notice,
        "debug" => Severity::Debug,
        _ => Severity::Info,
    }
}

/// What a line looks like it is, in this crate's vocabulary.
///
/// The judgement itself is [`stormcast::Severity::of`], shared with the wire
/// and with `stormpump`, so a line has one severity everywhere it appears
/// rather than one per program that looked at it.
pub fn severity_of(line: &str) -> Severity {
    match stormcast::Severity::of(line) {
        stormcast::Severity::Emergency => Severity::Emergency,
        stormcast::Severity::Alert => Severity::Alert,
        stormcast::Severity::Critical => Severity::Critical,
        stormcast::Severity::Error => Severity::Error,
        stormcast::Severity::Warning => Severity::Warning,
        stormcast::Severity::Notice => Severity::Notice,
        stormcast::Severity::Info => Severity::Info,
        stormcast::Severity::Debug => Severity::Debug,
    }
}

/// One archived run of a process.
#[derive(Debug, Clone, Serialize)]
pub struct RunInfo {
    pub run_id: String,
    pub process: String,
    /// The run id rendered as a date, for a console that shows it to a person.
    pub date: String,
    pub size_bytes: u64,
    /// How the run ended: `exited`, `failed`, or `current` for the live file.
    pub outcome: String,
}

/// The log directory, read-only.
pub struct LogStore {
    dir: PathBuf,
}

impl LogStore {
    pub fn new(dir: impl Into<PathBuf>) -> LogStore {
        LogStore { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Every file belonging to `process`, oldest first.
    ///
    /// Oldest first because a query is answered by reading forward and then
    /// tailing, and because the caller almost always wants time order.
    fn files_for(&self, process: &str) -> Vec<PathBuf> {
        let mut rotated: Vec<(u32, PathBuf)> = Vec::new();
        let mut archived: Vec<(String, PathBuf)> = Vec::new();
        let current = self.dir.join(format!("{process}.log"));

        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        for e in entries.flatten() {
            let path = e.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            let Some(rest) = name.strip_prefix(&format!("{process}.")) else { continue };
            let Some(rest) = rest.strip_suffix(".log") else { continue };
            if rest.is_empty() {
                continue;
            }
            match rest.parse::<u32>() {
                // `{process}.3.log` — a rotation generation, 1 being newest.
                Ok(n) => rotated.push((n, path)),
                // `{process}.20260825T115009.exited.log` — a finished run.
                Err(_) => archived.push((rest.to_owned(), path)),
            }
        }

        archived.sort_by(|a, b| a.0.cmp(&b.0));
        rotated.sort_by(|a, b| b.0.cmp(&a.0));

        let mut out: Vec<PathBuf> = archived.into_iter().map(|(_, p)| p).collect();
        out.extend(rotated.into_iter().map(|(_, p)| p));
        if current.exists() {
            out.push(current);
        }
        out
    }

    /// Which processes have logs here.
    pub fn processes(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return names };
        for e in entries.flatten() {
            let path = e.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            let Some(stem) = name.strip_suffix(".log") else { continue };
            // The process name is everything before the first dot, since
            // rotation and archiving both append dotted suffixes.
            let base = stem.split('.').next().unwrap_or(stem).to_owned();
            if !base.is_empty() && !names.contains(&base) {
                names.push(base);
            }
        }
        names.sort();
        names
    }

    /// Answer a query from the files.
    ///
    /// `tail` is applied last and defaults to something finite: a console that
    /// asks for "the logs" and is handed a hundred megabytes has hung, from the
    /// point of view of whoever is looking at it.
    pub fn query(&self, q: &LogQuery) -> Vec<LogEntry> {
        const DEFAULT_TAIL: usize = 1_000;

        let processes = match &q.process {
            Some(p) => vec![p.clone()],
            None => self.processes(),
        };

        let mut out: Vec<LogEntry> = Vec::new();
        for process in processes {
            for path in self.files_for(&process) {
                // A run filter is answerable from the file name alone, so a
                // query for one run does not read the others at all.
                if let Some(want) = &q.run_id {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    let is_run = name.contains(&format!(".{want}."));
                    let is_current = name == format!("{process}.log");
                    if !is_run && !is_current {
                        continue;
                    }
                }
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                for raw in text.lines() {
                    if raw.is_empty() {
                        continue;
                    }
                    let entry = parse_line(&process, raw);
                    if !matches(&entry, q) {
                        continue;
                    }
                    out.push(entry);
                }
            }
        }

        out.sort_by_key(|e| e.timestamp);
        let tail = q.tail.unwrap_or(DEFAULT_TAIL);
        if out.len() > tail {
            out.drain(..out.len() - tail);
        }
        out
    }

    /// The runs of a process, newest first.
    pub fn runs(&self, process: &str) -> Vec<RunInfo> {
        let mut runs: Vec<RunInfo> = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return runs };
        for e in entries.flatten() {
            let path = e.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            let Some(rest) = name.strip_prefix(&format!("{process}.")) else { continue };
            let Some(rest) = rest.strip_suffix(".log") else { continue };
            // `{run_id}.{outcome}` — anything else is a rotation generation.
            let Some((run_id, outcome)) = rest.rsplit_once('.') else { continue };
            if run_id.parse::<u32>().is_ok() {
                continue;
            }
            runs.push(RunInfo {
                run_id: run_id.to_owned(),
                process: process.to_owned(),
                date: pretty_run_id(run_id),
                size_bytes: e.metadata().map(|m| m.len()).unwrap_or(0),
                outcome: outcome.to_owned(),
            });
        }
        runs.sort_by(|a, b| b.run_id.cmp(&a.run_id));

        // The live file first, when there is one: the run someone is most
        // likely asking about is the one happening now.
        let current = self.dir.join(format!("{process}.log"));
        if let Ok(m) = std::fs::metadata(&current) {
            runs.insert(
                0,
                RunInfo {
                    run_id: "current".into(),
                    process: process.to_owned(),
                    date: "running".into(),
                    size_bytes: m.len(),
                    outcome: "current".into(),
                },
            );
        }
        runs
    }

    /// Keep the newest `keep` archived runs of a process and delete the rest.
    ///
    /// The log directory is a volume with a size, and a process that restarts
    /// in a loop writes one archive per restart. Without this the thing that
    /// fills the volume is the record of what went wrong.
    pub fn prune(&self, process: &str, keep: usize) -> usize {
        let runs = self.runs(process);
        let mut removed = 0;
        for r in runs.into_iter().filter(|r| r.outcome != "current").skip(keep) {
            let name = format!("{}.{}.{}.log", process, r.run_id, r.outcome);
            if std::fs::remove_file(self.dir.join(name)).is_ok() {
                removed += 1;
            }
        }
        removed
    }
}

/// `20260825T115009` as something a person reads.
fn pretty_run_id(run_id: &str) -> String {
    let b = run_id.as_bytes();
    if b.len() != 15 || b[8] != b'T' {
        return run_id.to_owned();
    }
    let s = |a: usize, z: usize| std::str::from_utf8(&b[a..z]).unwrap_or("");
    format!(
        "{}-{}-{} {}:{}:{}",
        s(0, 4),
        s(4, 6),
        s(6, 8),
        s(9, 11),
        s(11, 13),
        s(13, 15)
    )
}

fn matches(e: &LogEntry, q: &LogQuery) -> bool {
    if let Some(s) = q.stream {
        if e.stream != s {
            return false;
        }
    }
    // Severity is ordered the syslog way — 0 is the worst — so "at least this
    // severe" is `<=`, which reads backwards and is right.
    if let Some(min) = q.severity_min {
        if e.severity > min {
            return false;
        }
    }
    if let Some(since) = q.since {
        if e.timestamp < since {
            return false;
        }
    }
    if let Some(until) = q.until {
        if e.timestamp > until {
            return false;
        }
    }
    if let Some(needle) = &q.search {
        if !e.line.to_lowercase().contains(&needle.to_lowercase()) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stormlog-store-{tag}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_line_survives_the_round_trip() {
        let mut e = LogEntry::new("api", LogStream::Stderr, "it broke — badly");
        e.severity = Severity::Error;
        let raw = format_line(&e);
        let back = parse_line("api", raw.trim_end());
        assert_eq!(back.line, "it broke — badly");
        assert_eq!(back.severity, Severity::Error);
        assert_eq!(back.stream, LogStream::Stderr);
        // To the millisecond, which is what the format keeps.
        assert_eq!(
            back.timestamp.timestamp_millis(),
            e.timestamp.timestamp_millis()
        );
    }

    #[test]
    fn an_unparsable_line_is_kept_whole() {
        // Something else wrote to the file. The line is evidence either way.
        let e = parse_line("api", "PANIC: kernel BUG at fs/ext4/inode.c");
        assert!(e.line.contains("kernel BUG"));
        // A panic is worse than an error, and stormcast says so.
        assert_eq!(e.severity, Severity::Critical);
    }

    #[test]
    fn a_query_reads_the_files_in_time_order() {
        let dir = temp_dir("query");
        // An archived run, a rotation, and the live file.
        std::fs::write(
            dir.join("api.20260825T100000.exited.log"),
            "2026-08-25T10:00:00.000Z stdout info first\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("api.1.log"),
            "2026-08-25T11:00:00.000Z stdout info second\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("api.log"),
            "2026-08-25T12:00:00.000Z stderr error third\n",
        )
        .unwrap();

        let store = LogStore::new(&dir);
        let all = store.query(&LogQuery { process: Some("api".into()), ..Default::default() });
        assert_eq!(
            all.iter().map(|e| e.line.as_str()).collect::<Vec<_>>(),
            vec!["first", "second", "third"]
        );

        // Filters
        let errs = store.query(&LogQuery {
            process: Some("api".into()),
            severity_min: Some(Severity::Error),
            ..Default::default()
        });
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].line, "third");

        let found = store.query(&LogQuery {
            process: Some("api".into()),
            search: Some("SECOND".into()),
            ..Default::default()
        });
        assert_eq!(found.len(), 1);

        let tailed = store
            .query(&LogQuery { process: Some("api".into()), tail: Some(1), ..Default::default() });
        assert_eq!(tailed.len(), 1);
        assert_eq!(tailed[0].line, "third");
    }

    #[test]
    fn runs_are_listed_newest_first_with_the_live_one_ahead() {
        let dir = temp_dir("runs");
        std::fs::write(dir.join("api.20260825T100000.exited.log"), "x\n").unwrap();
        std::fs::write(dir.join("api.20260825T120000.failed.log"), "y\n").unwrap();
        // A rotation generation is not a run.
        std::fs::write(dir.join("api.1.log"), "z\n").unwrap();
        std::fs::write(dir.join("api.log"), "w\n").unwrap();

        let runs = LogStore::new(&dir).runs("api");
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].outcome, "current");
        assert_eq!(runs[1].run_id, "20260825T120000");
        assert_eq!(runs[1].outcome, "failed");
        assert_eq!(runs[1].date, "2026-08-25 12:00:00");
        assert_eq!(runs[2].run_id, "20260825T100000");
    }

    #[test]
    fn pruning_keeps_the_newest_and_never_the_live_file() {
        let dir = temp_dir("prune");
        for h in ["10", "11", "12"] {
            std::fs::write(dir.join(format!("api.202608{h}T100000.exited.log")), "x\n").unwrap();
        }
        std::fs::write(dir.join("api.log"), "live\n").unwrap();

        let store = LogStore::new(&dir);
        assert_eq!(store.prune("api", 1), 2);
        assert!(dir.join("api.log").exists());
        assert_eq!(store.runs("api").iter().filter(|r| r.outcome == "exited").count(), 1);
    }
}
