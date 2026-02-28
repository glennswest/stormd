use crate::config::BackupConfig;
use crate::events::{EventBus, EventKind};
use flate2::write::GzEncoder;
use flate2::Compression;
use std::path::Path;
use std::sync::Arc;
use tracing::{error, info};

pub struct BackupManager {
    config: BackupConfig,
    event_bus: Arc<EventBus>,
}

impl BackupManager {
    pub fn new(config: BackupConfig, event_bus: Arc<EventBus>) -> Self {
        Self { config, event_bus }
    }

    /// Archive the log directory and ship to the configured destination.
    pub async fn backup_logs(&self, log_dir: &Path) -> anyhow::Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        let dest = self
            .config
            .destination_url
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("backup destination_url not configured"))?;

        info!(log_dir = %log_dir.display(), dest = %dest, "starting log backup");
        self.event_bus
            .emit_simple(EventKind::BackupStarted, None)
            .await;

        // Create tar.gz archive in memory
        let archive_bytes = tokio::task::spawn_blocking({
            let log_dir = log_dir.to_path_buf();
            let compress = self.config.compress;
            move || create_archive(&log_dir, compress)
        })
        .await??;

        info!(
            size_bytes = archive_bytes.len(),
            "log archive created"
        );

        // Ship via HTTP POST
        let client = reqwest::Client::new();
        let content_type = if self.config.compress {
            "application/gzip"
        } else {
            "application/x-tar"
        };

        let mut req = client
            .post(dest)
            .header("Content-Type", content_type)
            .body(archive_bytes);

        for (k, v) in &self.config.headers {
            req = req.header(k.as_str(), v.as_str());
        }

        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                info!(dest = %dest, "log backup uploaded successfully");
                self.event_bus
                    .emit_simple(EventKind::BackupCompleted, None)
                    .await;
                Ok(())
            }
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                error!(status = %status, body = %body, "backup upload failed");
                self.event_bus
                    .emit_simple(EventKind::BackupFailed, None)
                    .await;
                anyhow::bail!("backup upload returned {}: {}", status, body)
            }
            Err(e) => {
                error!(error = %e, dest = %dest, "backup upload error");
                self.event_bus
                    .emit_simple(EventKind::BackupFailed, None)
                    .await;
                anyhow::bail!("backup upload failed: {}", e)
            }
        }
    }
}

fn create_archive(log_dir: &Path, compress: bool) -> anyhow::Result<Vec<u8>> {
    let buf = Vec::new();

    if compress {
        let encoder = GzEncoder::new(buf, Compression::default());
        let mut archive = tar::Builder::new(encoder);
        archive.append_dir_all("logs", log_dir)?;
        let encoder = archive.into_inner()?;
        Ok(encoder.finish()?)
    } else {
        let mut archive = tar::Builder::new(buf);
        archive.append_dir_all("logs", log_dir)?;
        Ok(archive.into_inner()?)
    }
}
