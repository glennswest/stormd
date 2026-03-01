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
    #[cfg(feature = "nats")]
    nats_client: tokio::sync::RwLock<Option<async_nats::Client>>,
    http_client: reqwest::Client,
}

impl EventBus {
    pub fn new(config: EventsConfig, container_name: String) -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            config,
            container_name,
            tx,
            #[cfg(feature = "nats")]
            nats_client: tokio::sync::RwLock::new(None),
            http_client: reqwest::Client::new(),
        }
    }

    pub async fn connect(&self) -> anyhow::Result<()> {
        #[cfg(feature = "nats")]
        if matches!(
            self.config.transport,
            EventTransport::Nats | EventTransport::Both
        ) {
            if let Some(url) = &self.config.nats_url {
                let client = async_nats::connect(url).await?;
                *self.nats_client.write().await = Some(client);
                info!(url = %url, "connected to NATS");
            }
        }
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

        // NATS publish
        #[cfg(feature = "nats")]
        if matches!(
            self.config.transport,
            EventTransport::Nats | EventTransport::Both
        ) {
            if let Some(client) = self.nats_client.read().await.as_ref() {
                let subject = format!("{}.{}", self.config.nats_subject, event.kind);
                if let Err(e) = client
                    .publish(subject, json.clone().into())
                    .await
                {
                    error!(error = %e, "NATS publish failed");
                }
            }
        }

        // Webhook POST
        if matches!(
            self.config.transport,
            EventTransport::Webhook | EventTransport::Both
        ) {
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
}
