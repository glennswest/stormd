use crate::types::{LogEntry, LogQuery, MinioConfig};
use s3::bucket::Bucket;
use s3::creds::Credentials;
use s3::region::Region;
use std::collections::VecDeque;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

const FLUSH_INTERVAL_SECS: u64 = 5;
const FLUSH_THRESHOLD: usize = 100;

/// MinIO S3 log storage backend.
pub struct LogStorage {
    config: MinioConfig,
    bucket: Option<Bucket>,
    buffer: Mutex<VecDeque<LogEntry>>,
}

impl LogStorage {
    pub fn new(config: MinioConfig) -> Self {
        Self {
            config,
            bucket: None,
            buffer: Mutex::new(VecDeque::new()),
        }
    }

    /// Initialize connection to MinIO and ensure the bucket exists.
    pub async fn init(&mut self) -> anyhow::Result<()> {
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

        self.bucket = Some(*bucket);
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
        let bucket = match &self.bucket {
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

        // Group entries by date/process/stream
        let mut groups: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for entry in &entries {
            let date = entry.timestamp.format("%Y-%m-%d").to_string();
            let key = format!("{}/{}/{}.jsonl", date, entry.process, entry.stream);
            let line = serde_json::to_string(&entry).unwrap_or_default();
            groups.entry(key).or_default().push(line);
        }

        let count = entries.len();

        for (key, lines) in groups {
            let content = lines.join("\n") + "\n";

            // Append to existing object (read + append + write)
            let existing = match bucket.get_object(&key).await {
                Ok(resp) => {
                    let bytes = resp.bytes();
                    String::from_utf8_lossy(bytes).to_string()
                }
                Err(_) => String::new(),
            };

            let full_content = existing + &content;
            if let Err(e) = bucket
                .put_object(&key, full_content.as_bytes())
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
        let bucket = match &self.bucket {
            Some(b) => b,
            None => return Ok(Vec::new()),
        };

        // List objects with optional prefix filter
        let prefix = match &query.process {
            Some(p) => format!("/{}", p),
            None => String::new(),
        };

        let results = bucket.list(prefix, Some("/".to_string())).await?;
        let mut entries = Vec::new();

        for list in results {
            for object in list.contents {
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

    /// Run the periodic flush loop.
    pub async fn run_flush_loop(&self) {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(FLUSH_INTERVAL_SECS)).await;
            let should_flush = {
                let buf = self.buffer.lock().await;
                buf.len() >= FLUSH_THRESHOLD || !buf.is_empty()
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
