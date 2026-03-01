use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize)]
pub struct SystemStats {
    pub container_name: String,
    pub started_at: DateTime<Utc>,
    pub uptime_secs: i64,
    pub pid: u32,
    pub process_count: usize,
    pub running_count: usize,
    pub failed_count: usize,
    pub total_restarts: u32,
    pub memory: Option<MemoryInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryInfo {
    pub rss_bytes: u64,
    pub vms_bytes: u64,
}

pub struct StatsCollector {
    container_name: String,
    started_at: DateTime<Utc>,
    process_stats: Arc<RwLock<ProcessStats>>,
}

#[derive(Debug, Default)]
struct ProcessStats {
    process_count: usize,
    running_count: usize,
    failed_count: usize,
    total_restarts: u32,
}

impl StatsCollector {
    pub fn new(container_name: String) -> Self {
        Self {
            container_name,
            started_at: Utc::now(),
            process_stats: Arc::new(RwLock::new(ProcessStats::default())),
        }
    }

    pub async fn update_process_stats(
        &self,
        total: usize,
        running: usize,
        failed: usize,
        restarts: u32,
    ) {
        let mut stats = self.process_stats.write().await;
        stats.process_count = total;
        stats.running_count = running;
        stats.failed_count = failed;
        stats.total_restarts = restarts;
    }

    pub async fn get_stats(&self) -> SystemStats {
        let ps = self.process_stats.read().await;
        SystemStats {
            container_name: self.container_name.clone(),
            started_at: self.started_at,
            uptime_secs: (Utc::now() - self.started_at).num_seconds(),
            pid: std::process::id(),
            process_count: ps.process_count,
            running_count: ps.running_count,
            failed_count: ps.failed_count,
            total_restarts: ps.total_restarts,
            memory: read_memory_info(),
        }
    }
}

fn read_memory_info() -> Option<MemoryInfo> {
    // Linux: parse /proc/self/status
    #[cfg(target_os = "linux")]
    {
        let content = std::fs::read_to_string("/proc/self/status").ok()?;
        let mut rss = 0u64;
        let mut vms = 0u64;
        for line in content.lines() {
            if let Some(val) = line.strip_prefix("VmRSS:") {
                rss = parse_kb(val);
            } else if let Some(val) = line.strip_prefix("VmSize:") {
                vms = parse_kb(val);
            }
        }
        Some(MemoryInfo {
            rss_bytes: rss * 1024,
            vms_bytes: vms * 1024,
        })
    }

    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg(target_os = "linux")]
fn parse_kb(s: &str) -> u64 {
    s.trim()
        .trim_end_matches("kB")
        .trim()
        .parse::<u64>()
        .unwrap_or(0)
}
