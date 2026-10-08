//! Kubernetes-shaped events for the processes stormd runs (stormd#48).
//!
//! Every probe failure, kill, back-off and start is recorded with upstream's
//! reasons and wording (`Unhealthy`, `Killing`, `BackOff`, `Created`,
//! `Started`), de-duplicated the way the kubelet does it: the same reason
//! and message again bumps `count` and `lastTimestamp`. `GET /api/v1/events`
//! serves them; rustkube-node publishes them on the process's mirror pod, so
//! they show in `kubectl describe pod` / `kubectl get events`.

use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Mutex;

/// Events kept, oldest dropped first.
const KEEP: usize = 1000;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    /// `Normal` or `Warning`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub reason: &'static str,
    pub message: String,
    /// The process (container) it is about.
    pub process: String,
    pub count: u64,
    pub first_timestamp: DateTime<Utc>,
    pub last_timestamp: DateTime<Utc>,
    /// Increases with every new or bumped event, for `?since=`.
    pub seq: u64,
}

#[derive(Default)]
pub struct EventStore {
    inner: Mutex<(VecDeque<Event>, u64)>,
}

impl EventStore {
    pub fn normal(&self, process: &str, reason: &'static str, message: impl Into<String>) {
        self.add("Normal", process, reason, message.into(), Utc::now());
    }

    pub fn warning(&self, process: &str, reason: &'static str, message: impl Into<String>) {
        self.add("Warning", process, reason, message.into(), Utc::now());
    }

    fn add(&self, kind: &'static str, process: &str, reason: &'static str, message: String, now: DateTime<Utc>) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.1 += 1;
        let seq = g.1;
        if let Some(e) = g.0.iter_mut().rev().find(|e| e.process == process && e.reason == reason && e.message == message) {
            e.count += 1;
            e.last_timestamp = now;
            e.seq = seq;
            return;
        }
        g.0.push_back(Event {
            kind,
            reason,
            message,
            process: process.to_string(),
            count: 1,
            first_timestamp: now,
            last_timestamp: now,
            seq,
        });
        while g.0.len() > KEEP {
            g.0.pop_front();
        }
    }

    /// Events changed after `since` (a `seq`), oldest change first.
    pub fn since(&self, since: u64) -> Vec<Event> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut v: Vec<Event> = g.0.iter().filter(|e| e.seq > since).cloned().collect();
        v.sort_by_key(|e| e.seq);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_are_counted_not_duplicated() {
        let s = EventStore::default();
        s.normal("fastetcd", "Started", "Started container fastetcd");
        s.warning("fastetcd", "Unhealthy", "Startup probe failed: dial tcp 127.0.0.1:2379: connection refused");
        s.warning("fastetcd", "Unhealthy", "Startup probe failed: dial tcp 127.0.0.1:2379: connection refused");
        s.warning("fastetcd", "Unhealthy", "Liveness probe failed: x");
        let all = s.since(0);
        assert_eq!(all.len(), 3);
        let u = all.iter().find(|e| e.message.starts_with("Startup")).unwrap();
        assert_eq!((u.count, u.kind), (2, "Warning"));
        assert_eq!(all.last().unwrap().message, "Liveness probe failed: x", "oldest change first");
        let seq = all.last().unwrap().seq;
        assert!(s.since(seq).is_empty());
        s.warning("fastetcd", "Unhealthy", "Startup probe failed: dial tcp 127.0.0.1:2379: connection refused");
        let changed = s.since(seq);
        assert_eq!((changed.len(), changed[0].count), (1, 3), "a bump shows up as a change");
        let json = serde_json::to_value(&changed[0]).unwrap();
        assert!(json.get("firstTimestamp").is_some() && json["type"] == "Warning");
    }
}
