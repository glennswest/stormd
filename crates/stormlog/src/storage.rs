use crate::types::{LogEntry, LogQuery, MinioConfig};
use s3::bucket::Bucket;
use s3::creds::Credentials;
use s3::region::Region;
use std::collections::VecDeque;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

const FLUSH_INTERVAL_SECS: u64 = 5;

/// MinIO S3 log storage backend.
pub struct LogStorage {
    config: MinioConfig,
    bucket: Mutex<Option<Bucket>>,
    buffer: Mutex<VecDeque<LogEntry>>,
}

impl LogStorage {
    pub fn new(config: MinioConfig) -> Self {
        Self {
            config,
            bucket: Mutex::new(None),
            buffer: Mutex::new(VecDeque::new()),
        }
    }

    /// Initialize connection to MinIO and ensure the bucket exists.
    pub async fn init(&self) -> anyhow::Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        let region = Region::Custom {
            region: self.config.region.clone(),
            endpoint: self.config.endpoint.clone(),
        };
        let credentials = Credentials::new(
            Some(&self.config.access_key),
            Some(&self.config.secret_key),
            None,
            None,
            None,
        )?;

        let mut bucket = Bucket::new(&self.config.bucket, region.clone(), credentials.clone())?
            .with_path_style();

        // Try to check if bucket exists, create if not
        match bucket.head_object("/").await {
            Ok(_) => {}
            Err(_) => {
                info!(bucket = %self.config.bucket, "creating log bucket");
                match Bucket::create_with_path_style(
                    &self.config.bucket,
                    region,
                    credentials,
                    s3::BucketConfiguration::default(),
                )
                .await
                {
                    Ok(resp) => {
                        bucket = resp.bucket;
                        bucket = bucket.with_path_style();
                    }
                    Err(e) => {
                        warn!(error = %e, "bucket creation failed (may already exist)");
                    }
                }
            }
        }

        *self.bucket.lock().await = Some(*bucket);
        info!(endpoint = %self.config.endpoint, bucket = %self.config.bucket, "MinIO storage initialized");
        Ok(())
    }

    /// Buffer a log entry for writing.
    pub async fn buffer_entry(&self, entry: LogEntry) {
        let mut buf = self.buffer.lock().await;
        buf.push_back(entry);
    }

    /// Flush buffered entries to MinIO.
    pub async fn flush(&self) -> anyhow::Result<usize> {
        let bucket_guard = self.bucket.lock().await;
        let bucket = match bucket_guard.as_ref() {
            Some(b) => b,
            None => return Ok(0),
        };

        let entries: Vec<LogEntry> = {
            let mut buf = self.buffer.lock().await;
            buf.drain(..).collect()
        };

        if entries.is_empty() {
            return Ok(0);
        }

        // Group entries by date/process/run_id/stream
        let mut groups: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for entry in &entries {
            let date = entry.timestamp.format("%Y-%m-%d").to_string();
            let run_part = entry
                .run_id
                .as_deref()
                .unwrap_or("default");
            let key = format!("{}/{}/{}/{}.jsonl", date, entry.process, run_part, entry.stream);
            let line = serde_json::to_string(&entry).unwrap_or_default();
            groups.entry(key).or_default().push(line);
        }

        let count = entries.len();

        for (key, lines) in &groups {
            let content = lines.join("\n") + "\n";

            // Append to existing object (read + append + write)
            let existing = match bucket.get_object(key).await {
                Ok(resp) => {
                    let bytes = resp.bytes();
                    String::from_utf8_lossy(bytes).to_string()
                }
                Err(_) => String::new(),
            };

            let full_content = existing + &content;
            if let Err(e) = bucket
                .put_object(key, full_content.as_bytes())
                .await
            {
                error!(key = %key, error = %e, "failed to write log object");
                // Re-buffer entries on failure
                let mut buf = self.buffer.lock().await;
                for entry in entries {
                    buf.push_back(entry);
                }
                return Err(e.into());
            }
        }

        Ok(count)
    }

    /// Query stored logs from MinIO.
    pub async fn query(&self, query: &LogQuery) -> anyhow::Result<Vec<LogEntry>> {
        let bucket_guard = self.bucket.lock().await;
        let bucket = match bucket_guard.as_ref() {
            Some(b) => b,
            None => return Ok(Vec::new()),
        };

        // Build prefix for listing
        let prefix = match (&query.process, &query.run_id) {
            (Some(p), Some(r)) => format!("/{}/{}", p, r),
            (Some(p), None) => format!("/{}", p),
            _ => String::new(),
        };

        let results = bucket.list(prefix, None).await?;
        let mut entries = Vec::new();

        for list in results {
            for object in list.contents {
                if !object.key.ends_with(".jsonl") {
                    continue;
                }
                let resp = match bucket.get_object(&object.key).await {
                    Ok(r) => r,
                    Err(_) => continue,
                };

                let content = String::from_utf8_lossy(resp.bytes());
                for line in content.lines() {
                    if line.is_empty() {
                        continue;
                    }
                    if let Ok(entry) = serde_json::from_str::<LogEntry>(line) {
                        // Apply filters
                        if let Some(ref stream) = query.stream {
                            if entry.stream != *stream {
                                continue;
                            }
                        }
                        if let Some(ref sev) = query.severity_min {
                            if entry.severity > *sev {
                                continue;
                            }
                        }
                        if let Some(ref since) = query.since {
                            if entry.timestamp < *since {
                                continue;
                            }
                        }
                        if let Some(ref until) = query.until {
                            if entry.timestamp > *until {
                                continue;
                            }
                        }
                        if let Some(ref search) = query.search {
                            if !entry.line.contains(search.as_str()) {
                                continue;
                            }
                        }
                        entries.push(entry);
                    }
                }
            }
        }

        // Sort by timestamp
        entries.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

        // Apply tail
        if let Some(tail) = query.tail {
            let start = entries.len().saturating_sub(tail);
            entries = entries[start..].to_vec();
        }

        Ok(entries)
    }

    /// List all run IDs for a process, newest first.
    pub async fn list_runs(&self, process: &str) -> anyhow::Result<Vec<RunInfo>> {
        let bucket_guard = self.bucket.lock().await;
        let bucket = match bucket_guard.as_ref() {
            Some(b) => b,
            None => return Ok(Vec::new()),
        };

        let prefix = format!("/{}", process);
        let results = bucket.list(prefix, None).await?;

        let mut runs: std::collections::HashMap<String, RunInfo> =
            std::collections::HashMap::new();

        for list in results {
            for object in list.contents {
                // Key format: {date}/{process}/{run_id}/{stream}.jsonl
                let parts: Vec<&str> = object.key.split('/').collect();
                if parts.len() >= 4 {
                    let run_id = parts[parts.len() - 2].to_string();
                    let date = parts[0].to_string();
                    let entry = runs.entry(run_id.clone()).or_insert_with(|| RunInfo {
                        run_id,
                        process: process.to_string(),
                        date,
                        size_bytes: 0,
                        object_count: 0,
                    });
                    entry.size_bytes += object.size as u64;
                    entry.object_count += 1;
                }
            }
        }

        let mut runs: Vec<RunInfo> = runs.into_values().collect();
        runs.sort_by(|a, b| b.run_id.cmp(&a.run_id)); // newest first
        Ok(runs)
    }

    /// Run the periodic flush loop.
    pub async fn run_flush_loop(&self) {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(FLUSH_INTERVAL_SECS)).await;
            let should_flush = {
                let buf = self.buffer.lock().await;
                !buf.is_empty()
            };
            if should_flush {
                if let Err(e) = self.flush().await {
                    error!(error = %e, "flush to MinIO failed, entries re-buffered");
                }
            }
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }
}

/// Metadata about a process run stored in MinIO.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RunInfo {
    pub run_id: String,
    pub process: String,
    pub date: String,
    pub size_bytes: u64,
    pub object_count: u32,
}
