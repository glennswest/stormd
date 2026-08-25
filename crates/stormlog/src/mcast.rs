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
use std::net::SocketAddr;

pub use stormcast::{strip_ansi, DEFAULT_GROUP};

pub struct Emitter {
    inner: stormcast::Emitter,
}

impl Emitter {
    pub fn new(addr: SocketAddr, host: impl Into<String>) -> Option<Emitter> {
        stormcast::Emitter::new(addr, host).map(|inner| Emitter { inner })
    }

    /// Send an entry, carrying the time it happened rather than the time it is
    /// sent — a backlog forwarded after the network came up would otherwise
    /// collapse onto one instant.
    pub fn send(&self, entry: &LogEntry) {
        let ts = entry.timestamp.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string();
        self.inner.send_at(&ts, &entry.process, severity_of(entry.severity), &entry.line);
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
