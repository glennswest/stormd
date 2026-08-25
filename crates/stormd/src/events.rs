use crate::config::{EventTransport, EventsConfig};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;
use tokio::sync::broadcast;
use tracing::{error, info};

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
    http_client: reqwest::Client,
}

impl EventBus {
    pub fn new(config: EventsConfig, container_name: String) -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            config,
            container_name,
            tx,
            log: tokio::sync::RwLock::new(None),
            http_client: reqwest::Client::new(),
        }
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
                    line.push_str(&format!(" {k}={v}"));
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

        if !self.config.enabled {
            return;
        }

        let json = match serde_json::to_vec(&event) {
            Ok(j) => j,
            Err(e) => {
                error!(error = %e, "failed to serialize event");
                return;
            }
        };


        // Webhook POST
        if self.config.transport == EventTransport::Webhook {
            if let Some(url) = &self.config.webhook_url {
                let mut req = self.http_client.post(url).json(&event);
                for (k, v) in &self.config.webhook_headers {
                    req = req.header(k.as_str(), v.as_str());
                }
                if let Err(e) = req.send().await {
                    error!(error = %e, url = %url, "webhook POST failed");
                }
            }
        }
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
