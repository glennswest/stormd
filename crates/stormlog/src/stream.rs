use crate::types::LogEntry;
use std::collections::HashMap;
use tokio::sync::{broadcast, Mutex};

const CHANNEL_CAPACITY: usize = 1024;

/// Broadcast multiplexer — per-process channels plus a global channel.
pub struct StreamManager {
    global_tx: broadcast::Sender<LogEntry>,
    process_channels: Mutex<HashMap<String, broadcast::Sender<LogEntry>>>,
}

impl StreamManager {
    pub fn new() -> Self {
        let (global_tx, _) = broadcast::channel(CHANNEL_CAPACITY);
        Self {
            global_tx,
            process_channels: Mutex::new(HashMap::new()),
        }
    }

    /// Publish a log entry to both the per-process and global channels.
    pub async fn publish(&self, entry: LogEntry) {
        // Per-process channel
        let mut channels = self.process_channels.lock().await;
        let tx = channels
            .entry(entry.process.clone())
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0);
        let _ = tx.send(entry.clone());
        drop(channels);

        // Global channel
        let _ = self.global_tx.send(entry);
    }

    /// Subscribe to a specific process's log stream.
    pub async fn subscribe_process(&self, process: &str) -> broadcast::Receiver<LogEntry> {
        let mut channels = self.process_channels.lock().await;
        let tx = channels
            .entry(process.to_string())
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0);
        tx.subscribe()
    }

    /// Subscribe to the global log stream (all processes).
    pub fn subscribe_all(&self) -> broadcast::Receiver<LogEntry> {
        self.global_tx.subscribe()
    }
}
