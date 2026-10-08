//! Emit to the fleet's multicast group.
//!
//! The framing itself lives in [`stormcast`], shared with `stormpump` — one
//! wire format for every process on a node, whether the host's PID 1 or a
//! container's is supervising it. Two implementations of one format drift, and
//! the drift shows up as a viewer that cannot read a node.
//!
//! What is here is the adapter: stormlog's own [`LogEntry`] and severity onto
//! that wire.

use crate::types::{LogEntry, Severity};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Instant;

pub use stormcast::{strip_ansi, DEFAULT_GROUP};
use stormcast::{Limiter, Verdict, BURST, RATE_PER_SEC};

pub struct Emitter {
    inner: stormcast::Emitter,
    gate: Gate,
}

impl Emitter {
    pub fn new(addr: SocketAddr, host: impl Into<String>) -> Option<Emitter> {
        stormcast::Emitter::new(addr, host).map(|inner| Emitter { inner, gate: Gate::new(RATE_PER_SEC, BURST) })
    }

    /// Send an entry through the limiter, carrying the time it happened
    /// rather than the time it is sent — a backlog forwarded after the
    /// network came up would otherwise collapse onto one instant.
    pub fn send(&self, entry: &LogEntry) {
        let ts = entry.timestamp.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string();
        for (sev, line) in self.gate.offer(entry, Instant::now()) {
            self.inner.send_at(&ts, &entry.process, severity_of(sev), &line);
        }
    }

    /// A process's output has ended: send a run of repeats still held.
    pub fn flush(&self, process: &str) {
        if let Some(n) = self.gate.flush(process) {
            let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string();
            self.inner.send_at(&ts, process, severity_of(Severity::Notice), &n);
        }
    }
}

/// stormcast's limiter, one per process (stormd#12).
///
/// **Every line went to the group.** A process looping on one line, or
/// printing thousands a second, put each one on the fleet's multicast group —
/// stormcast#1's failure (10,920 copies of one line at 27/s, ingested by every
/// collector) — because stormpump limits on the host and containers did not.
/// Now: repeats collapse to `last message repeated N time(s)`, more than
/// `RATE_PER_SEC` lines a second (after a `BURST`) are dropped with a count,
/// the same judgement and notices as stormpump. Only the group is limited;
/// the file, the terminal and the streams keep every line.
struct Gate {
    per: Mutex<HashMap<String, Limiter>>,
    rate: f64,
    burst: f64,
}

impl Gate {
    fn new(rate: f64, burst: f64) -> Gate {
        Gate { per: Mutex::new(HashMap::new()), rate, burst }
    }

    /// What to send for `entry`, in order: due notices (as Notice), then the
    /// line itself if the limiter lets it through. stormd's own crash marker
    /// (Emergency) is never rate-dropped, so a flood cannot hide the crash.
    fn offer(&self, entry: &LogEntry, now: Instant) -> Vec<(Severity, String)> {
        let mut per = self.per.lock().unwrap_or_else(|e| e.into_inner());
        let lim = per
            .entry(entry.process.clone())
            .or_insert_with(|| Limiter::new(self.rate, self.burst));
        let (notices, verdict) = lim.offer(&entry.line, now);
        let mut out: Vec<(Severity, String)> = notices.into_iter().map(|n| (Severity::Notice, n)).collect();
        if verdict == Verdict::Emit || (verdict == Verdict::Dropped && entry.severity == Severity::Emergency) {
            out.push((entry.severity, entry.line.clone()));
        }
        out
    }

    fn flush(&self, process: &str) -> Option<String> {
        self.per.lock().unwrap_or_else(|e| e.into_inner()).get_mut(process)?.flush()
    }
}

/// stormlog's severities onto the wire's.
fn severity_of(s: Severity) -> stormcast::Severity {
    match s {
        Severity::Emergency => stormcast::Severity::Emergency,
        Severity::Alert => stormcast::Severity::Alert,
        Severity::Critical => stormcast::Severity::Critical,
        Severity::Error => stormcast::Severity::Error,
        Severity::Warning => stormcast::Severity::Warning,
        Severity::Notice => stormcast::Severity::Notice,
        Severity::Info => stormcast::Severity::Info,
        Severity::Debug => stormcast::Severity::Debug,
    }
}

#[cfg(test)]
mod gate_tests {
    use super::Gate;
    use crate::types::{LogEntry, LogStream, Severity};
    use std::time::{Duration, Instant};

    fn e(process: &str, line: &str) -> LogEntry {
        LogEntry::new(process, LogStream::Stdout, line)
    }

    fn lines(v: Vec<(Severity, String)>) -> Vec<String> {
        v.into_iter().map(|(_, l)| l).collect()
    }

    /// stormcast#1's failure: one line, looped. One copy goes out, then a
    /// count when something else is said.
    #[test]
    fn a_looping_line_is_collapsed() {
        let g = Gate::new(200.0, 2000.0);
        let t = Instant::now();
        assert_eq!(lines(g.offer(&e("app", "retrying"), t)), vec!["retrying"]);
        for _ in 0..10_919 {
            assert!(g.offer(&e("app", "retrying"), t).is_empty());
        }
        let out = g.offer(&e("app", "connected"), t);
        assert_eq!(out[0], (Severity::Notice, "last message repeated 10919 time(s)".to_string()));
        assert_eq!(out[1].1, "connected");
    }

    #[test]
    fn a_flood_is_rate_limited_with_a_count_and_sources_are_separate() {
        let g = Gate::new(10.0, 5.0);
        let t = Instant::now();
        let sent: usize = (0..100).map(|i| g.offer(&e("noisy", &format!("line {i}")), t).len()).sum();
        assert_eq!(sent, 5, "only the burst goes out at once");
        // Another process has its own bucket.
        assert_eq!(lines(g.offer(&e("quiet", "hello"), t)), vec!["hello"]);
        // A second later the bucket has refilled: the count comes first.
        let out = g.offer(&e("noisy", "after"), t + Duration::from_secs(1));
        assert_eq!(out[0].1, "95 message(s) dropped — over 10 lines/s");
        assert_eq!(out.last().unwrap().1, "after");
    }

    #[test]
    fn the_crash_marker_is_never_rate_dropped_and_flush_reports_repeats() {
        let g = Gate::new(10.0, 1.0);
        let t = Instant::now();
        g.offer(&e("app", "first"), t);
        assert!(g.offer(&e("app", "second"), t).is_empty(), "over the rate");
        let crash = e("app", "*** PROCESS CRASHED *** exit code 1").with_severity(Severity::Emergency);
        assert_eq!(lines(g.offer(&crash, t)), vec!["*** PROCESS CRASHED *** exit code 1"]);

        g.offer(&e("svc", "tick"), t);
        g.offer(&e("svc", "tick"), t);
        g.offer(&e("svc", "tick"), t);
        assert_eq!(g.flush("svc").as_deref(), Some("last message repeated 2 time(s)"));
        assert_eq!(g.flush("svc"), None);
        assert_eq!(g.flush("never-seen"), None);
    }
}

#[cfg(test)]
mod wire_tests {
    use super::Emitter;
    use crate::types::{LogEntry, LogStream};

    /// stormd#28 (stormcast#4): a line whose byte 8192 falls inside a
    /// multibyte character is cut at a character boundary, not a panic in
    /// PID 1.
    #[test]
    fn a_long_multibyte_line_is_sent_not_a_panic() {
        let rx = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        rx.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
        let em = Emitter::new(rx.local_addr().unwrap(), "node-1").expect("emitter");
        let line = "a".repeat(8191) + &"é".repeat(100);
        em.send(&LogEntry::new("app", LogStream::Stdout, line));
        let mut buf = vec![0u8; 65536];
        let n = rx.recv(&mut buf).expect("a datagram");
        let text = std::str::from_utf8(&buf[..n]).expect("valid UTF-8 on the wire");
        assert!(text.contains(" app ") && text.contains("aaaa"), "{}", &text[..120]);
        assert!(n < 8192 + 200, "not cut: {n} bytes");
    }
}
