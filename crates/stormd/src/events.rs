use crate::config::{EventTransport, EventsConfig};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, warn};

/// Events waiting for the webhook. Past this, new ones are dropped (and
/// counted) rather than waited for (stormd#83).
pub const WEBHOOK_QUEUE: usize = 256;
/// How long one webhook POST may take to connect, and in all.
pub const WEBHOOK_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const WEBHOOK_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub kind: EventKind,
    pub process: Option<String>,
    pub container: String,
    pub detail: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    ContainerStarting,
    ContainerStopping,
    ContainerFailing,
    ProcessStarted,
    ProcessStopped,
    ProcessCrashed,
    ProcessRestarting,
    ProcessReady,
    CronExecuted,
    CronFailed,
    BackupStarted,
    BackupCompleted,
    BackupFailed,
    UpdateCheckStarted,
    UpdateAvailable,
    UpdatePulling,
    UpdatePivoting,
    UpdateCompleted,
    UpdateFailed,
    LivenessCheckFailed,
}

impl std::fmt::Display for EventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| format!("{:?}", self));
        write!(f, "{}", s)
    }
}

pub struct EventBus {
    config: EventsConfig,
    container_name: String,
    tx: broadcast::Sender<Event>,
    /// The log, once there is one.
    ///
    /// Every event goes here as well, and this is the path that needs no
    /// configuration: it reaches the file, the fleet's multicast group and
    /// anyone following, so "what restarted, where, and how often" is one
    /// query against a collector rather than a thing each node keeps to
    /// itself. A webhook stays what it was — optional, for people who want
    /// events somewhere specific as well.
    ///
    /// Set after construction because the first event — the container
    /// starting — happens before there is a log to put it in.
    log: tokio::sync::RwLock<Option<std::sync::Arc<stormlog::StormLog>>>,
    /// The webhook's queue (stormd#83).
    ///
    /// **A webhook that never answered stopped supervision.** `emit` awaited
    /// the POST inline, with no timeout, and it is called from the middle of
    /// lifecycle work — `ContainerStarting` before the API and signal
    /// handlers, `ProcessStarted` before a child's monitor, restarts and
    /// stops. One optional receiver that accepted connections and never sent
    /// headers held all of it. Now `emit` only queues: one task delivers, in
    /// order, each POST bounded by `WEBHOOK_CONNECT_TIMEOUT` and
    /// `WEBHOOK_REQUEST_TIMEOUT`, and a full queue drops the newest event
    /// with a count rather than wait.
    webhook_tx: Option<mpsc::Sender<Event>>,
    /// Taken by the first `emit`, which starts the delivery task (inside the
    /// runtime: `new` may run before one exists).
    webhook_rx: std::sync::Mutex<Option<mpsc::Receiver<Event>>>,
    webhook_client: reqwest::Client,
    /// Events dropped because the queue was full.
    webhook_dropped: AtomicU64,
    /// A drop has been logged since the queue last had room: one WARN per
    /// outage, not one per event. Cleared by the delivery task.
    webhook_drop_logged: std::sync::Arc<AtomicBool>,
}

impl EventBus {
    pub fn new(config: EventsConfig, container_name: String) -> Self {
        Self::with_timeouts(config, container_name, WEBHOOK_CONNECT_TIMEOUT, WEBHOOK_REQUEST_TIMEOUT)
    }

    fn with_timeouts(config: EventsConfig, container_name: String, connect: Duration, request: Duration) -> Self {
        let (tx, _) = broadcast::channel(256);
        let webhook = config.enabled && config.transport == EventTransport::Webhook && config.webhook_url.is_some();
        let (webhook_tx, webhook_rx) = if webhook {
            let (t, r) = mpsc::channel(WEBHOOK_QUEUE);
            (Some(t), Some(r))
        } else {
            (None, None)
        };
        Self {
            config,
            container_name,
            tx,
            log: tokio::sync::RwLock::new(None),
            webhook_tx,
            webhook_rx: std::sync::Mutex::new(webhook_rx),
            webhook_client: reqwest::Client::builder()
                .connect_timeout(connect)
                .timeout(request)
                .build()
                .unwrap_or_default(),
            webhook_dropped: AtomicU64::new(0),
            webhook_drop_logged: std::sync::Arc::new(AtomicBool::new(false)),
        }
    }

    /// Events the webhook queue has dropped since stormd started.
    pub fn webhook_dropped(&self) -> u64 {
        self.webhook_dropped.load(Ordering::Relaxed)
    }

    /// Queue an event for the webhook; never waits (stormd#83).
    fn queue_webhook(&self, event: &Event) {
        let Some(tx) = &self.webhook_tx else { return };
        if let Some(rx) = self.webhook_rx.lock().unwrap_or_else(|e| e.into_inner()).take() {
            self.start_delivery(rx);
        }
        if tx.try_send(event.clone()).is_err() {
            let n = self.webhook_dropped.fetch_add(1, Ordering::Relaxed) + 1;
            if !self.webhook_drop_logged.swap(true, Ordering::Relaxed) {
                warn!(dropped_total = n, queue = WEBHOOK_QUEUE, "webhook queue full — dropping events until it drains");
            }
        }
    }

    /// The one task that POSTs queued events, in order.
    fn start_delivery(&self, mut rx: mpsc::Receiver<Event>) {
        let Some(url) = self.config.webhook_url.clone() else { return };
        let headers = self.config.webhook_headers.clone();
        let client = self.webhook_client.clone();
        let drop_logged = self.webhook_drop_logged.clone();
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                // Taking one made room: the next drop is a new outage.
                drop_logged.store(false, Ordering::Relaxed);
                let mut req = client.post(&url).json(&event);
                for (k, v) in &headers {
                    req = req.header(k.as_str(), v.as_str());
                }
                match req.send().await {
                    Ok(r) if !r.status().is_success() => {
                        warn!(url = %url, status = %r.status(), kind = %event.kind, "webhook refused the event")
                    }
                    Ok(_) => {}
                    Err(e) => error!(error = %e, url = %url, kind = %event.kind, "webhook POST failed"),
                }
            }
        });
    }

    pub async fn connect(&self) -> anyhow::Result<()> {
        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    pub async fn emit(&self, kind: EventKind, process: Option<String>, detail: HashMap<String, serde_json::Value>) {
        let event = Event {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            kind,
            process,
            container: self.container_name.clone(),
            detail,
        };

        let _ = self.tx.send(event.clone());

        // To the log first, and unconditionally.
        //
        // An event that is only delivered when someone configured a transport
        // is an event that is missing exactly on the node nobody had set up —
        // and a crash loop nobody can see is the case this exists for.
        if let Some(log) = self.log.read().await.as_ref() {
            let mut line = format!("event={}", event.kind);
            if let Some(p) = &event.process {
                line.push_str(&format!(" process={p}"));
            }
            line.push_str(&format!(" container={}", event.container));
            let mut keys: Vec<&String> = event.detail.keys().collect();
            keys.sort();
            for k in keys {
                if let Some(v) = event.detail.get(k) {
                    // A string renders bare rather than quoted: this is a log
                    // line, not JSON, and `restart="3"` reads worse than
                    // `restart=3` for no gain.
                    match v.as_str() {
                        Some(t) => line.push_str(&format!(" {k}={t}")),
                        None => line.push_str(&format!(" {k}={v}")),
                    }
                }
            }
            let entry = stormlog::types::LogEntry::new(
                "event",
                stormlog::types::LogStream::Ingest,
                line,
            )
            .with_severity(severity_of(&event.kind));
            log.write_entry(entry).await;
        }

        // The webhook, if any: queued, never waited for (stormd#83).
        self.queue_webhook(&event);
    }

    pub async fn emit_simple(&self, kind: EventKind, process: Option<String>) {
        self.emit(kind, process, HashMap::new()).await;
    }

    /// Give the bus somewhere to write. Called once the log exists.
    pub async fn set_log(&self, log: std::sync::Arc<stormlog::StormLog>) {
        *self.log.write().await = Some(log);
    }
}

/// How loudly an event should read.
///
/// A restart is a warning and not an error: it is the supervisor doing its
/// job, and a viewer filtered to errors should show the crash that caused it
/// rather than the response to it.
fn severity_of(kind: &EventKind) -> stormlog::types::Severity {
    use stormlog::types::Severity as S;
    match kind {
        EventKind::ContainerFailing => S::Critical,
        EventKind::ProcessCrashed
        | EventKind::CronFailed
        | EventKind::BackupFailed
        | EventKind::UpdateFailed => S::Error,
        EventKind::ProcessRestarting
        | EventKind::LivenessCheckFailed
        | EventKind::ContainerStopping
        | EventKind::ProcessStopped => S::Warning,
        _ => S::Info,
    }
}

/// stormd#83 (3b808d13a1d6): a webhook never holds up `emit`.
#[cfg(test)]
mod webhook_tests {
    use super::{EventBus, EventKind, WEBHOOK_QUEUE};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    fn config(url: &str) -> crate::config::EventsConfig {
        toml::from_str(&format!("enabled = true\ntransport = \"webhook\"\nwebhook_url = \"{url}\"\n")).unwrap()
    }

    /// A webhook that accepts connections and never answers.
    async fn hung() -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/events", l.local_addr().unwrap());
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((s, _)) = l.accept().await {
                held.push(s);
            }
        });
        url
    }

    /// Emitting stays instant with the webhook hung: the queue fills, the
    /// rest are dropped and counted, and nothing waits on it.
    #[tokio::test]
    async fn a_hung_webhook_never_holds_up_emit() {
        let bus = EventBus::new(config(&hung().await), "t".into());
        let t = Instant::now();
        for _ in 0..(WEBHOOK_QUEUE + 100) {
            bus.emit_simple(EventKind::ProcessStarted, Some("p".into())).await;
        }
        let took = t.elapsed();
        assert!(took < Duration::from_secs(2), "{} emits took {took:?}", WEBHOOK_QUEUE + 100);
        assert!(bus.webhook_dropped() >= 99, "dropped {}", bus.webhook_dropped());
    }

    /// Each POST is bounded: a hung one is given up after the request
    /// timeout, and the next event is still delivered.
    #[tokio::test]
    async fn a_hung_post_times_out_and_delivery_goes_on() {
        // Answers the second connection only.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/events", l.local_addr().unwrap());
        let got = Arc::new(Mutex::new(Vec::<String>::new()));
        let g = got.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (first, _) = l.accept().await.unwrap();
            let (mut second, _) = l.accept().await.unwrap();
            // Headers and body may come in separate reads.
            let mut req = Vec::new();
            let mut buf = vec![0u8; 8192];
            while !String::from_utf8_lossy(&req).contains("\"second\"") {
                let n = second.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&buf[..n]);
            }
            g.lock().unwrap().push(String::from_utf8_lossy(&req).into_owned());
            second.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await.unwrap();
            drop(first);
        });
        let bus = EventBus::with_timeouts(config(&url), "t".into(), Duration::from_secs(1), Duration::from_millis(500));
        bus.emit_simple(EventKind::ProcessStarted, Some("first".into())).await;
        bus.emit_simple(EventKind::ProcessCrashed, Some("second".into())).await;
        for _ in 0..60 {
            if !got.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let got = got.lock().unwrap().clone();
        assert_eq!(got.len(), 1, "the event after a hung POST was never delivered");
        assert!(got[0].contains("process_crashed") && got[0].contains("second"), "{}", got[0]);
    }

    /// Delivered in the order emitted.
    #[tokio::test]
    async fn events_arrive_in_order() {
        use axum::{extract::State, routing::post, Json, Router};
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        async fn take(State(s): State<Arc<Mutex<Vec<String>>>>, Json(e): Json<serde_json::Value>) {
            s.lock().unwrap().push(e["process"].as_str().unwrap_or("").to_string());
        }
        let app = Router::new().route("/events", post(take)).with_state(seen.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/events", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        let bus = EventBus::new(config(&url), "t".into());
        for i in 0..20 {
            bus.emit_simple(EventKind::ProcessStarted, Some(format!("p{i}"))).await;
        }
        for _ in 0..100 {
            if seen.lock().unwrap().len() == 20 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let want: Vec<String> = (0..20).map(|i| format!("p{i}")).collect();
        assert_eq!(*seen.lock().unwrap(), want);
        assert_eq!(bus.webhook_dropped(), 0);
    }
}
