use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::RwLock;

const MEMORY_HISTORY_MAX: usize = 360; // 30 minutes at 5-second intervals

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

#[derive(Debug, Clone, Serialize)]
pub struct MemorySample {
    pub timestamp: DateTime<Utc>,
    pub rss_bytes: u64,
    pub vms_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MountInfo {
    pub device: String,
    pub mount_point: String,
    pub fs_type: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub avail_bytes: u64,
    pub use_percent: f64,
}

pub struct StatsCollector {
    container_name: String,
    started_at: DateTime<Utc>,
    process_stats: Arc<RwLock<ProcessStats>>,
    memory_history: Arc<RwLock<VecDeque<MemorySample>>>,
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
            memory_history: Arc::new(RwLock::new(VecDeque::with_capacity(MEMORY_HISTORY_MAX))),
        }
    }

    /// Start the background memory sampling loop (call once).
    pub fn start_memory_monitor(self: &Arc<Self>) {
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                if let Some(mem) = read_memory_info() {
                    let sample = MemorySample {
                        timestamp: Utc::now(),
                        rss_bytes: mem.rss_bytes,
                        vms_bytes: mem.vms_bytes,
                    };
                    let mut history = this.memory_history.write().await;
                    if history.len() >= MEMORY_HISTORY_MAX {
                        history.pop_front();
                    }
                    history.push_back(sample);
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        });
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

    pub async fn get_memory_history(&self) -> Vec<MemorySample> {
        let history = self.memory_history.read().await;
        history.iter().cloned().collect()
    }

    pub fn get_mounts() -> Vec<MountInfo> {
        read_mount_info()
    }
}

fn read_memory_info() -> Option<MemoryInfo> {
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

fn read_mount_info() -> Vec<MountInfo> {
    #[cfg(target_os = "linux")]
    {
        let content = match std::fs::read_to_string("/proc/mounts") {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let mut mounts = Vec::new();
        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 3 {
                continue;
            }
            let device = parts[0];
            let mount_point = parts[1];
            let fs_type = parts[2];

            // Skip pseudo-filesystems
            if matches!(
                fs_type,
                "proc" | "sysfs" | "devpts" | "tmpfs" | "cgroup" | "cgroup2"
                    | "securityfs" | "debugfs" | "pstore" | "bpf" | "tracefs"
                    | "hugetlbfs" | "mqueue" | "fusectl" | "configfs"
            ) && !mount_point.starts_with("/dev/shm")
            {
                continue;
            }

            // Skip kernel virtual mounts
            if device == "none" || device == "proc" || device == "sysfs" {
                continue;
            }

            if let Some(info) = statvfs_info(mount_point, device, fs_type) {
                mounts.push(info);
            }
        }
        mounts
    }

    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

#[cfg(target_os = "linux")]
fn statvfs_info(mount_point: &str, device: &str, fs_type: &str) -> Option<MountInfo> {
    use std::ffi::CString;
    let path = CString::new(mount_point).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::statvfs(path.as_ptr(), &mut stat) };
    if ret != 0 {
        return None;
    }
    let block_size = stat.f_frsize as u64;
    let total = stat.f_blocks as u64 * block_size;
    let avail = stat.f_bavail as u64 * block_size;
    let free = stat.f_bfree as u64 * block_size;
    let used = total.saturating_sub(free);
    let use_pct = if total > 0 {
        (used as f64 / total as f64) * 100.0
    } else {
        0.0
    };
    // Skip zero-size filesystems
    if total == 0 {
        return None;
    }
    Some(MountInfo {
        device: device.to_string(),
        mount_point: mount_point.to_string(),
        fs_type: fs_type.to_string(),
        total_bytes: total,
        used_bytes: used,
        avail_bytes: avail,
        use_percent: (use_pct * 10.0).round() / 10.0,
    })
}

#[cfg(target_os = "linux")]
fn parse_kb(s: &str) -> u64 {
    s.trim()
        .trim_end_matches("kB")
        .trim()
        .parse::<u64>()
        .unwrap_or(0)
}
