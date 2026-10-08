//! Health of the APIs each process serves (stormd#49, stormcos#458).
//!
//! Liveness asks a cheap `/healthz`. That answers while the process's real
//! work is stuck: on 2026-10-08 a storage engine held its volume mutex
//! through a six-minute template build, every API call behind it stalled,
//! and nothing at the OS level noticed (stormblock#358). So a process can
//! declare its real APIs (`[[process.api]]`, a cheap real read), and stormd
//! times them against a budget: `healthy`, `slow`, `stalled`, `down`. Every
//! change is logged loudly and kept; acting on it (a restart) is opt-in.

use crate::config::ApiProbe;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tracing::{error, info, warn};

/// How many recent answers the p50/p99 are taken over.
const WINDOW: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiState {
    Unknown,
    Healthy,
    Slow,
    Stalled,
    Down,
}

/// What one probe saw.
#[derive(Debug, Clone, PartialEq)]
pub enum Probe {
    /// An answer, in this many ms, with this HTTP status.
    Answered { ms: u64, status: u16 },
    /// No answer within the timeout.
    Timeout,
    /// Refused, reset, unresolvable: the error.
    Error(String),
}

/// The state a probe puts an API in, given its recent answers (this one
/// included) and its budgets.
pub fn classify(probe: &Probe, window: &VecDeque<u64>, p50_ms: Option<u64>, p99_ms: Option<u64>) -> ApiState {
    match probe {
        Probe::Timeout => ApiState::Stalled,
        Probe::Error(_) => ApiState::Down,
        // A 4xx is a probe the process refuses (a 401: the probe's
        // credentials are wrong): saying so beats calling it healthy.
        Probe::Answered { status, .. } if *status >= 400 => ApiState::Down,
        Probe::Answered { ms, .. } => {
            let over_p99 = p99_ms.is_some_and(|b| *ms > b);
            let over_p50 = p50_ms.is_some_and(|b| percentile(window, 50).is_some_and(|p| p > b));
            if over_p99 || over_p50 {
                ApiState::Slow
            } else {
                ApiState::Healthy
            }
        }
    }
}

/// The `pct`th percentile of `v` (nearest rank).
pub fn percentile(v: &VecDeque<u64>, pct: usize) -> Option<u64> {
    if v.is_empty() {
        return None;
    }
    let mut s: Vec<u64> = v.iter().copied().collect();
    s.sort_unstable();
    let rank = (pct * s.len()).div_ceil(100).max(1);
    Some(s[rank - 1])
}

/// One API's health, as served by `GET /api/v1/health/apis`.
#[derive(Debug, Clone, Serialize)]
pub struct ApiHealth {
    pub process: String,
    pub api: String,
    pub url: String,
    pub state: ApiState,
    pub since: DateTime<Utc>,
    pub last_ms: Option<u64>,
    pub p50_ms: Option<u64>,
    pub p99_ms: Option<u64>,
    pub budget_p50_ms: Option<u64>,
    pub budget_p99_ms: Option<u64>,
    pub last_error: Option<String>,
    pub last_check: Option<DateTime<Utc>>,
    pub checks: u64,
    #[serde(skip)]
    window: VecDeque<u64>,
}

/// Every API's health, kept across process restarts.
pub struct ApiHealthStore {
    map: Mutex<HashMap<(String, String), ApiHealth>>,
    /// `<dir>/<process>.jsonl` gets each change, when `<dir>`'s parent exists.
    history_dir: PathBuf,
}

impl Default for ApiHealthStore {
    fn default() -> Self {
        Self::new(PathBuf::from("/system-data/history/api"))
    }
}

impl ApiHealthStore {
    pub fn new(history_dir: PathBuf) -> Self {
        Self { map: Mutex::new(HashMap::new()), history_dir }
    }

    /// Record a probe. Returns the API's state now, and since when.
    pub fn record(&self, process: &str, api: &ApiProbe, probe: Probe, now: DateTime<Utc>) -> (ApiState, DateTime<Utc>) {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        let h = map.entry((process.to_string(), api.name.clone())).or_insert_with(|| ApiHealth {
            process: process.to_string(),
            api: api.name.clone(),
            url: api.url.clone(),
            state: ApiState::Unknown,
            since: now,
            last_ms: None,
            p50_ms: None,
            p99_ms: None,
            budget_p50_ms: api.p50_ms,
            budget_p99_ms: api.p99_ms,
            last_error: None,
            last_check: None,
            checks: 0,
            window: VecDeque::new(),
        });
        h.checks += 1;
        h.last_check = Some(now);
        match &probe {
            Probe::Answered { ms, .. } => {
                h.last_ms = Some(*ms);
                h.window.push_back(*ms);
                if h.window.len() > WINDOW {
                    h.window.pop_front();
                }
                h.p50_ms = percentile(&h.window, 50);
                h.p99_ms = percentile(&h.window, 99);
            }
            Probe::Timeout => h.last_ms = None,
            Probe::Error(_) => h.last_ms = None,
        }
        h.last_error = match &probe {
            Probe::Answered { status, .. } if *status >= 400 => Some(format!("HTTP {status}")),
            Probe::Answered { .. } => None,
            Probe::Timeout => Some(format!("no answer within {} s", api.timeout_secs)),
            Probe::Error(e) => Some(e.clone()),
        };
        let state = classify(&probe, &h.window, api.p50_ms, api.p99_ms);
        if state != h.state {
            let was = h.state;
            let lasted = (now - h.since).num_seconds().max(0);
            h.state = state;
            h.since = now;
            self.say(h, was, lasted);
            self.keep(h, was, lasted);
        }
        (h.state, h.since)
    }

    /// Log a change once, loudly in proportion.
    fn say(&self, h: &ApiHealth, was: ApiState, lasted: i64) {
        let (p, a, url) = (&h.process, &h.api, &h.url);
        let ms = h.last_ms;
        let err = h.last_error.as_deref().unwrap_or("");
        match h.state {
            ApiState::Healthy => info!(process = %p, api = %a, %url, latency_ms = ?ms, was = ?was, was_secs = lasted, "API healthy"),
            ApiState::Slow => warn!(
                process = %p, api = %a, %url, latency_ms = ?ms, p50_ms = ?h.p50_ms,
                budget_p50_ms = ?h.budget_p50_ms, budget_p99_ms = ?h.budget_p99_ms,
                was = ?was, was_secs = lasted, "API slow — over its latency budget"
            ),
            ApiState::Stalled => error!(process = %p, api = %a, %url, error = %err, was = ?was, was_secs = lasted, "API STALLED — no answer"),
            ApiState::Down => error!(process = %p, api = %a, %url, error = %err, was = ?was, was_secs = lasted, "API DOWN"),
            ApiState::Unknown => {}
        }
    }

    /// Append the change to the history file, when the history volume is
    /// mounted (stormcos#456). Best-effort: a failure is a debug line.
    fn keep(&self, h: &ApiHealth, was: ApiState, lasted: i64) {
        if !self.history_dir.parent().is_some_and(|d| d.is_dir()) {
            return;
        }
        let line = serde_json::json!({
            "ts": h.since, "process": h.process, "api": h.api, "url": h.url,
            "from": was, "to": h.state, "from_secs": lasted,
            "latency_ms": h.last_ms, "p50_ms": h.p50_ms, "p99_ms": h.p99_ms, "error": h.last_error,
        });
        let path = self.history_dir.join(format!("{}.jsonl", h.process));
        let r = std::fs::create_dir_all(&self.history_dir).and_then(|_| {
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
            writeln!(f, "{line}")
        });
        if let Err(e) = r {
            tracing::debug!(path = %path.display(), error = %e, "API history not kept");
        }
    }

    pub fn list(&self) -> Vec<ApiHealth> {
        let mut v: Vec<ApiHealth> = self.map.lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect();
        v.sort_by(|a, b| (&a.process, &a.api).cmp(&(&b.process, &b.api)));
        v
    }
}

/// The client one API is probed with: its timeout, its client certificate
/// if it has one, certificates not verified (the supervisor knows what it
/// started; the question is whether it answers in time).
pub fn client(api: &ApiProbe) -> anyhow::Result<reqwest::Client> {
    let mut b = reqwest::Client::builder()
        .timeout(Duration::from_secs(api.timeout_secs.max(1)))
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::none());
    if let (Some(c), Some(k)) = (&api.client_cert_file, &api.client_key_file) {
        let mut pem = std::fs::read(c)?;
        pem.extend_from_slice(b"\n");
        pem.extend(std::fs::read(k)?);
        b = b.identity(reqwest::Identity::from_pem(&pem)?);
    }
    Ok(b.build()?)
}

/// Probe once: GET the URL, time the answer.
pub async fn probe(client: &reqwest::Client, api: &ApiProbe) -> Probe {
    let mut req = client.get(&api.url);
    if let Some(f) = &api.token_file {
        if let Ok(t) = std::fs::read_to_string(f) {
            if !t.trim().is_empty() {
                req = req.bearer_auth(t.trim());
            }
        }
    }
    let started = std::time::Instant::now();
    match req.send().await {
        Ok(r) => {
            let status = r.status().as_u16();
            // The body is part of the answer: a server that sends headers and
            // then stalls is stalled.
            match r.bytes().await {
                Ok(_) => Probe::Answered { ms: started.elapsed().as_millis() as u64, status },
                Err(e) if e.is_timeout() => Probe::Timeout,
                Err(e) => Probe::Error(e.to_string()),
            }
        }
        Err(e) if e.is_timeout() => Probe::Timeout,
        Err(e) => Probe::Error(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(t: &str) -> ApiProbe {
        toml::from_str(&format!("name = \"volumes\"\nurl = \"http://127.0.0.1:1/api/v1/volumes?limit=1\"\n{t}")).unwrap()
    }

    fn w(v: &[u64]) -> VecDeque<u64> {
        v.iter().copied().collect()
    }

    #[test]
    fn states_from_one_probe() {
        let ok = |ms| Probe::Answered { ms, status: 200 };
        assert_eq!(classify(&ok(20), &w(&[20]), Some(50), Some(200)), ApiState::Healthy);
        assert_eq!(classify(&ok(250), &w(&[20, 250]), Some(50), Some(200)), ApiState::Slow, "over p99");
        assert_eq!(classify(&ok(60), &w(&[60, 70, 80]), Some(50), Some(200)), ApiState::Slow, "p50 over budget");
        assert_eq!(classify(&ok(5000), &w(&[5000]), None, None), ApiState::Healthy, "no budget, no slow");
        assert_eq!(classify(&Probe::Timeout, &w(&[]), None, None), ApiState::Stalled);
        assert_eq!(classify(&Probe::Error("refused".into()), &w(&[]), None, None), ApiState::Down);
        assert_eq!(classify(&Probe::Answered { ms: 3, status: 401 }, &w(&[3]), None, None), ApiState::Down);
        assert_eq!(classify(&Probe::Answered { ms: 3, status: 503 }, &w(&[3]), None, None), ApiState::Down);
    }

    #[test]
    fn percentiles_nearest_rank() {
        let v = w(&[10, 20, 30, 40, 50, 60, 70, 80, 90, 100]);
        assert_eq!(percentile(&v, 50), Some(50));
        assert_eq!(percentile(&v, 99), Some(100));
        assert_eq!(percentile(&w(&[7]), 50), Some(7));
        assert_eq!(percentile(&w(&[]), 50), None);
    }

    /// A change is recorded once, with how long the state before lasted, and
    /// kept in the history file when its volume is there.
    #[test]
    fn changes_are_recorded_and_kept() {
        let base = std::env::temp_dir().join(format!("stormd-apih-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let store = ApiHealthStore::new(base.join("api"));
        let a = api("p99_ms = 100\n");
        let t0 = Utc::now();
        let ok = Probe::Answered { ms: 10, status: 200 };
        assert_eq!(store.record("stormblock", &a, ok.clone(), t0).0, ApiState::Healthy);
        assert_eq!(store.record("stormblock", &a, ok.clone(), t0 + chrono::Duration::seconds(15)).1, t0, "no change, same since");
        let t1 = t0 + chrono::Duration::seconds(360);
        assert_eq!(store.record("stormblock", &a, Probe::Timeout, t1), (ApiState::Stalled, t1));
        assert_eq!(store.record("stormblock", &a, ok, t1 + chrono::Duration::seconds(30)).0, ApiState::Healthy);

        let h = &store.list()[0];
        assert_eq!((h.checks, h.last_ms, h.last_error.as_deref()), (4, Some(10), None));
        let text = std::fs::read_to_string(base.join("api/stormblock.jsonl")).unwrap();
        let lines: Vec<serde_json::Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(lines.len(), 3, "unknown→healthy, healthy→stalled, stalled→healthy: {text}");
        assert_eq!((lines[1]["from"].as_str(), lines[1]["to"].as_str(), lines[1]["from_secs"].as_i64()), (Some("healthy"), Some("stalled"), Some(360)));
        assert_eq!(lines[1]["error"].as_str(), Some("no answer within 5 s"));
    }

    #[test]
    fn no_history_volume_no_file() {
        let store = ApiHealthStore::new(PathBuf::from("/nonexistent-stormd-49/history/api"));
        store.record("p", &api(""), Probe::Error("refused".into()), Utc::now());
        assert!(!std::path::Path::new("/nonexistent-stormd-49").exists());
        assert_eq!(store.list()[0].state, ApiState::Down);
    }

    /// Against real sockets: an answer is timed, a server that never answers
    /// is a timeout, a closed port is an error.
    #[tokio::test]
    async fn probes_see_answers_stalls_and_refusals() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let fast = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fast_port = fast.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = fast.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = s.read(&mut buf).await;
                    let _ = s.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok").await;
                });
            }
        });
        let stuck = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stuck_port = stuck.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            loop {
                let (s, _) = stuck.accept().await.unwrap();
                held.push(s); // accepted, never answered: a held mutex
            }
        });
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();

        let mk = |port: u16| -> ApiProbe {
            toml::from_str(&format!("name = \"x\"\nurl = \"http://127.0.0.1:{port}/api/v1/volumes?limit=1\"\ntimeout_secs = 1\n")).unwrap()
        };
        let a = mk(fast_port);
        assert!(matches!(probe(&client(&a).unwrap(), &a).await, Probe::Answered { status: 200, .. }));
        let a = mk(stuck_port);
        let t = std::time::Instant::now();
        assert_eq!(probe(&client(&a).unwrap(), &a).await, Probe::Timeout);
        assert!(t.elapsed() < Duration::from_secs(3));
        let a = mk(closed);
        assert!(matches!(probe(&client(&a).unwrap(), &a).await, Probe::Error(_)));
    }
}
