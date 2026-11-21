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
    /// The last [`TAIL_LINES`] lines each process's current run printed
    /// (stdout and stderr together), for stormd to echo when it fails
    /// (stormd#29).
    tails: std::sync::Mutex<HashMap<String, std::collections::VecDeque<String>>>,
}

/// Lines kept per run for [`StormLog::tail`].
pub const TAIL_LINES: usize = 20;

impl StormLog {
    fn remember(&self, process: &str, line: &str) {
        let mut tails = self.tails.lock().unwrap_or_else(|e| e.into_inner());
        let t = tails.entry(process.to_string()).or_default();
        t.push_back(line.to_string());
        while t.len() > TAIL_LINES {
            t.pop_front();
        }
    }

    /// The last lines the process's current (or just-ended) run printed,
    /// oldest first.
    pub fn tail(&self, process: &str) -> Vec<String> {
        let tails = self.tails.lock().unwrap_or_else(|e| e.into_inner());
        tails.get(process).map(|t| t.iter().cloned().collect()).unwrap_or_default()
    }

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
        // The node, then this container's name. See `McastConfig::host`.
        //
        // A container gets its own UTS namespace, which is *copied* from the
        // parent's — so this reads the node's hostname without needing the
        // namespace shared.
        let host = config.mcast.host.clone().unwrap_or_else(|| {
            std::fs::read_to_string("/proc/sys/kernel/hostname")
                .map(|h| h.trim().to_owned())
                .ok()
                .filter(|h| !h.is_empty() && h != "localhost")
                .unwrap_or_else(|| name.clone())
        });
        let mcast = mcast_group(config.mcast.group.as_deref()).and_then(|addr| mcast::Emitter::new(addr, host));

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
            tails: std::sync::Mutex::new(HashMap::new()),
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
        // Readers of an earlier run still here (a stop that did not archive,
        // or a descendant holding its pipe) are drained briefly and then
        // ended, before this run begins, so they cannot write into it
        // (stormd#85).
        let earlier = self.reader_tasks.lock().await.remove(&process);
        if let Some(earlier) = earlier {
            Self::drain(&process, earlier, NEW_RUN_DRAIN).await;
        }

        let run_id = Self::make_run_id();
        info!(process = %process, run_id = %run_id, "starting log capture");

        // Store the run_id for this process
        {
            let mut ids = self.run_ids.lock().await;
            ids.insert(process.clone(), run_id.clone());
        }
        // A new run's tail starts empty.
        self.tails.lock().unwrap_or_else(|e| e.into_inner()).insert(process.clone(), Default::default());

        // Emit a marker entry for the run start
        let marker = LogEntry::new(&process, LogStream::Stdout, "--- process started ---")
            .with_run_id(&run_id);
        self.write_entry(marker).await;

        let mut handles: Vec<JoinHandle<()>> = Vec::new();

        if let Some(stdout) = stdout {
            let this = self.clone();
            let name = process.clone();
            let rid = run_id.clone();
            handles.push(tokio::spawn(async move {
                this.capture(&name, Some(&rid), stdout, LogStream::Stdout, Severity::Info, true).await;
                // Its output has ended: a run of repeats still held by the
                // group's limiter goes out now, not with the next run.
                if let Some(m) = &this.mcast {
                    m.flush(&name);
                }
                let end = LogEntry::new(&name, LogStream::Stdout, "--- process exited ---").with_run_id(&rid);
                this.write_entry(end).await;
            }));
        }

        if let Some(stderr) = stderr {
            let this = self.clone();
            let name = process.clone();
            let rid = run_id.clone();
            handles.push(tokio::spawn(async move {
                this.capture(&name, Some(&rid), stderr, LogStream::Stderr, Severity::Warning, true).await;
            }));
        }

        // Store handles so archive_run can await them.
        self.reader_tasks.lock().await.insert(process, handles);
    }

    /// Read one stream of a run into log records until EOF (stormd#85).
    ///
    /// **A record is a line, not a read.** A pipe keeps no write or line
    /// boundaries: each 4 KiB read used to be decoded and split on its own,
    /// so one diagnostic written in two writes, or longer than the buffer,
    /// became several entries, a UTF-8 character split across reads became
    /// U+FFFD, and severity was judged on fragments. Bytes now gather in a
    /// buffer, a record is everything up to `\n` (a trailing `\r` dropped),
    /// decoded whole; a line longer than `MAX_LINE` is cut at a character
    /// boundary into records of at most that; what is left at EOF is the
    /// last record. Empty lines are skipped, as before. The terminal still
    /// gets the raw bytes as they come.
    async fn capture<R>(&self, name: &str, run_id: Option<&str>, stream: R, kind: LogStream, default: Severity, terminal: bool)
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        let mut reader = BufReader::new(stream);
        let mut buf = [0u8; 4096];
        let mut pending: Vec<u8> = Vec::new();
        loop {
            match reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if terminal {
                        self.terminal_manager.feed(name, &buf[..n]).await;
                    }
                    pending.extend_from_slice(&buf[..n]);
                    for record in take_records(&mut pending, false) {
                        self.record(name, run_id, &record, kind, default).await;
                    }
                }
                Err(e) => {
                    error!(error = %e, process = %name, stream = ?kind, "read error");
                    break;
                }
            }
        }
        for record in take_records(&mut pending, true) {
            self.record(name, run_id, &record, kind, default).await;
        }
    }

    /// One captured line: remembered for the tail, written everywhere.
    async fn record(&self, name: &str, run_id: Option<&str>, line: &str, kind: LogStream, default: Severity) {
        if line.is_empty() {
            return;
        }
        let sev = detect_severity(line, default);
        self.remember(name, line);
        let mut entry = LogEntry::new(name, kind, line).with_severity(sev);
        if let Some(r) = run_id {
            entry = entry.with_run_id(r);
        }
        self.write_entry(entry).await;
    }

    /// Stream a side job's output — a cron run — into the log as it comes,
    /// line by line, until EOF (stormd#95). The same records as a process's
    /// capture (`MAX_LINE`, whole lines, whole characters), with no run and
    /// no terminal. Memory stays one 4 KiB read plus at most one unfinished
    /// line, however much the job writes.
    pub async fn capture_stream<R>(&self, name: &str, stream: R, kind: LogStream, default: Severity)
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        self.capture(name, None, stream, kind, default, false).await
    }

    /// Wait for a run's readers to drain, up to `deadline` in all; one still
    /// running then is aborted, and awaited, so it cannot outlive the run
    /// (stormd#85). Dropping a timed-out `JoinHandle` only detached it: a
    /// descendant holding the pipe kept it writing into the next run.
    /// One deadline for all of them: stdout and stderr both held open
    /// wait `deadline` in all, not once each.
    async fn drain(process: &str, handles: Vec<JoinHandle<()>>, deadline: std::time::Duration) {
        let until = tokio::time::Instant::now() + deadline;
        for mut h in handles {
            if tokio::time::timeout_at(until, &mut h).await.is_err() {
                warn!(
                    process = %process, deadline_secs = deadline.as_secs_f32(),
                    "output still open after the run ended (a child holding the pipe?) — capture ended"
                );
                h.abort();
                let _ = h.await;
            }
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
            Self::drain(process, handles, ARCHIVE_DRAIN).await;
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
    /// A run filter knows which run each live file is (stormd#98).
    pub async fn query_logs(&self, query: &LogQuery) -> anyhow::Result<Vec<LogEntry>> {
        let active = self.run_ids.lock().await.clone();
        Ok(self.store.query_active(query, &|p| active.get(p).cloned()))
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

/// The longest record a captured line makes; longer ones are cut into
/// records of at most this (stormd#85).
pub const MAX_LINE: usize = 64 * 1024;
/// How long `archive_run` lets a run's readers drain before ending them.
const ARCHIVE_DRAIN: std::time::Duration = std::time::Duration::from_secs(5);
/// How long a new run lets readers of the previous one, still running, drain.
const NEW_RUN_DRAIN: std::time::Duration = std::time::Duration::from_secs(1);

/// Take the complete records out of `pending`: each line up to `\n` (a
/// trailing `\r` dropped), a run of more than `MAX_LINE` bytes without one
/// cut at a UTF-8 boundary, and — at `eof` — whatever is left. Each record is
/// decoded whole, so a character split across reads stays one character.
fn take_records(pending: &mut Vec<u8>, eof: bool) -> Vec<String> {
    let mut out = Vec::new();
    loop {
        if let Some(nl) = pending.iter().position(|&b| b == b'\n') {
            if nl <= MAX_LINE {
                let mut line: Vec<u8> = pending.drain(..=nl).collect();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                out.push(String::from_utf8_lossy(&line).into_owned());
                continue;
            }
        }
        if pending.len() > MAX_LINE {
            // Cut at the last character boundary within MAX_LINE.
            let cut = match std::str::from_utf8(&pending[..MAX_LINE]) {
                Ok(_) => MAX_LINE,
                Err(e) if e.error_len().is_none() && e.valid_up_to() > 0 => e.valid_up_to(),
                Err(_) => MAX_LINE,
            };
            let chunk: Vec<u8> = pending.drain(..cut).collect();
            out.push(String::from_utf8_lossy(&chunk).into_owned());
            continue;
        }
        break;
    }
    if eof && !pending.is_empty() {
        let rest = std::mem::take(pending);
        out.push(String::from_utf8_lossy(&rest).trim_end_matches('\r').to_string());
    }
    out
}

/// Where lines go on the wire: no `group` → the fleet's default group;
/// `"off"` or `""` → nowhere; anything else → that `host:port` (nowhere, with
/// a warning, if it does not parse).
///
/// **`off` used to send to the fleet group** (stormd#27): it was filtered
/// out and then replaced by the default, so a container configured to be
/// quiet emitted like every other.
fn mcast_group(group: Option<&str>) -> Option<std::net::SocketAddr> {
    match group.map(str::trim) {
        None => mcast::DEFAULT_GROUP.parse().ok(),
        Some("") | Some("off") => None,
        Some(g) => match g.parse() {
            Ok(a) => Some(a),
            Err(e) => {
                tracing::warn!(group = %g, error = %e, "[stormlog.mcast] group is not host:port — not sending");
                None
            }
        },
    }
}

#[cfg(test)]
mod group_tests {
    use super::mcast_group;

    #[test]
    fn off_is_off() {
        assert_eq!(mcast_group(Some("off")), None);
        assert_eq!(mcast_group(Some("")), None);
        assert_eq!(mcast_group(None), Some("239.255.42.1:5514".parse().unwrap()));
        assert_eq!(mcast_group(Some("239.1.2.3:600")), Some("239.1.2.3:600".parse().unwrap()));
        assert_eq!(mcast_group(Some("not an address")), None);
    }
}

#[cfg(test)]
mod tail_tests {
    use super::{StormLog, TAIL_LINES};
    use std::sync::Arc;

    /// stormd#29: the last lines of a run, stdout and stderr, are there to
    /// echo once it has ended; a new run starts with an empty tail.
    #[tokio::test]
    async fn a_runs_last_lines_are_kept() {
        let dir = std::env::temp_dir().join(format!("stormlog-tail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut cfg = crate::types::StormLogConfig::default();
        cfg.file.log_dir = dir.clone();
        cfg.mcast.group = Some("off".into());
        let log = Arc::new(StormLog::new(cfg, "t"));
        let run = |script: &'static str| {
            let log = log.clone();
            async move {
                let mut child = tokio::process::Command::new("/bin/sh")
                    .args(["-c", script])
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .unwrap();
                log.spawn_capture("fastetcd".into(), child.stdout.take(), child.stderr.take()).await;
                child.wait().await.unwrap();
                log.archive_run("fastetcd", true).await;
            }
        };
        run("for i in $(seq 1 30); do echo line $i; done; echo 'Error: storage io: DB corrupted' >&2; exit 1").await;
        let t = log.tail("fastetcd");
        assert_eq!(t.len(), TAIL_LINES);
        assert_eq!(t.last().map(String::as_str), Some("Error: storage io: DB corrupted"));
        assert_eq!(t.first().map(String::as_str), Some("line 12"));
        run("echo only").await;
        assert_eq!(log.tail("fastetcd"), vec!["only".to_string()], "a new run starts empty");
        assert!(log.tail("never").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// stormd#85 (7b4d771dbcf5): records are lines, whatever the reads were.
#[cfg(test)]
mod record_tests {
    use super::{take_records, MAX_LINE};

    fn feed(chunks: &[&[u8]]) -> Vec<String> {
        let mut pending = Vec::new();
        let mut out = Vec::new();
        for c in chunks {
            pending.extend_from_slice(c);
            out.extend(take_records(&mut pending, false));
        }
        out.extend(take_records(&mut pending, true));
        out
    }

    #[test]
    fn a_line_split_across_reads_is_one_record() {
        assert_eq!(feed(&[b"Error: disk ", b"full\nnext\n"]), vec!["Error: disk full", "next"]);
        assert_eq!(feed(&[b"a\r", b"\nb\r\n"]), vec!["a", "b"], "CRLF, even split");
    }

    #[test]
    fn a_character_split_across_reads_stays_whole() {
        let e = "é".as_bytes();
        assert_eq!(feed(&[b"caf", &e[..1], &e[1..], b"\n"]), vec!["café"]);
    }

    #[test]
    fn a_partial_line_at_eof_is_the_last_record() {
        assert_eq!(feed(&[b"one\ntwo"]), vec!["one", "two"]);
        assert!(feed(&[b""]).is_empty());
    }

    #[test]
    fn a_long_line_is_cut_at_a_character_boundary() {
        // MAX_LINE - 1 ASCII bytes, then a 2-byte character straddling the cap.
        let mut line = vec![b'x'; MAX_LINE - 1];
        line.extend_from_slice("é".as_bytes());
        line.extend_from_slice(b"rest\n");
        let r = feed(&[&line]);
        assert_eq!(r.len(), 2, "{:?}", r.iter().map(String::len).collect::<Vec<_>>());
        assert_eq!(r[0].len(), MAX_LINE - 1, "cut before the character, not through it");
        assert_eq!(r[1], "érest");
        assert!(r.iter().all(|s| !s.contains('\u{FFFD}')));
    }
}

/// stormd#85 against real pipes: lines across writes, and a reader that
/// outlives its run is ended (618ee2a9a43d).
#[cfg(test)]
mod capture_tests {
    use super::StormLog;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn log(label: &str) -> (Arc<StormLog>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("stormlog-cap-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut cfg = crate::types::StormLogConfig::default();
        cfg.file.log_dir = dir.clone();
        cfg.mcast.group = Some("off".into());
        (Arc::new(StormLog::new(cfg, "t")), dir)
    }

    async fn run(log: &Arc<StormLog>, name: &str, script: &str) -> Duration {
        let mut child = tokio::process::Command::new("/bin/sh")
            .args(["-c", script])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        log.spawn_capture(name.into(), child.stdout.take(), child.stderr.take()).await;
        child.wait().await.unwrap();
        let t = Instant::now();
        log.archive_run(name, false).await;
        t.elapsed()
    }

    #[tokio::test]
    async fn lines_and_characters_survive_separate_writes() {
        let (log, dir) = log("lines");
        // "é" is \303\251; each half, and each half-line, in its own write.
        run(&log, "p", r"printf 'Error: half'; sleep 0.2; printf ' a line\n'; printf 'caf\303'; sleep 0.2; printf '\251\n'; printf 'no newline'").await;
        let t = log.tail("p");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(t, vec!["Error: half a line", "café", "no newline"]);
    }

    #[tokio::test]
    async fn a_reader_left_open_by_a_descendant_is_ended_with_its_run() {
        let (log, dir) = log("drain");
        // The background subshell keeps stdout open past the run, and
        // writes into it 6 s later.
        let waited = run(&log, "p", "(sleep 6; echo late) 2>/dev/null & echo hi").await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        let t = log.tail("p");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(waited >= Duration::from_secs(4), "archive did not wait for the drain: {waited:?}");
        assert!(waited < Duration::from_secs(8), "archive waited {waited:?}");
        assert_eq!(t, vec!["hi"], "the reader outlived its run and kept writing");
    }
}
