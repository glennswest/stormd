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

/// One supervised process's resource use, from `/proc/<pid>` (stormd#33).
/// The process itself only: what it forks is not added in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProcUsage {
    pub rss_bytes: u64,
    pub vm_bytes: u64,
    pub cpu_seconds: f64,
    pub open_fds: u64,
}

/// Read a process's usage; `None` when it has gone or `/proc` is unreadable.
pub fn proc_usage(pid: u32) -> Option<ProcUsage> {
    let dir = std::path::PathBuf::from(format!("/proc/{pid}"));
    let (rss_bytes, vm_bytes) = parse_status_memory(&std::fs::read_to_string(dir.join("status")).ok()?);
    let cpu_seconds = parse_stat_cpu_ticks(&std::fs::read_to_string(dir.join("stat")).ok()?)? as f64 / clock_ticks() as f64;
    let open_fds = std::fs::read_dir(dir.join("fd")).ok()?.count() as u64;
    Some(ProcUsage { rss_bytes, vm_bytes, cpu_seconds, open_fds })
}

/// `VmRSS` and `VmSize` from `/proc/<pid>/status`, in bytes. A kernel thread
/// or a zombie has neither: 0.
fn parse_status_memory(status: &str) -> (u64, u64) {
    let (mut rss, mut vm) = (0, 0);
    for line in status.lines() {
        if let Some(v) = line.strip_prefix("VmRSS:") {
            rss = parse_kb(v) * 1024;
        } else if let Some(v) = line.strip_prefix("VmSize:") {
            vm = parse_kb(v) * 1024;
        }
    }
    (rss, vm)
}

/// utime + stime (fields 14 and 15) from `/proc/<pid>/stat`, in clock
/// ticks. The command name (field 2) is in parentheses and may itself hold
/// spaces and parentheses, so fields are counted from the last `)`.
fn parse_stat_cpu_ticks(stat: &str) -> Option<u64> {
    let after = &stat[stat.rfind(')')? + 1..];
    let f: Vec<&str> = after.split_whitespace().collect();
    // f[0] is field 3 (state), so field 14 is f[11].
    Some(f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?)
}

fn clock_ticks() -> u64 {
    // SAFETY: sysconf reads a constant.
    let t = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if t > 0 { t as u64 } else { 100 }
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

            // Only show real filesystems — block devices and named mounts
            // that people actually care about in a container context
            let dominated = device.starts_with("/dev/")
                || fs_type == "ext4"
                || fs_type == "xfs"
                || fs_type == "btrfs"
                || fs_type == "zfs"
                || fs_type == "nfs"
                || fs_type == "nfs4"
                || fs_type == "cifs"
                || fs_type == "fuse"
                || fs_type == "overlay";
            if !dominated {
                continue;
            }

            // Skip k8s internal mounts that aren't interesting
            if mount_point.starts_with("/dev/termination-log")
                || mount_point.starts_with("/etc/hosts")
                || mount_point.starts_with("/etc/hostname")
                || mount_point.starts_with("/etc/resolv.conf")
                || mount_point.starts_with("/proc/")
                || mount_point.starts_with("/sys/")
            {
                continue;
            }

            // Deduplicate: skip if we already have a mount for the same mount_point
            if mounts.iter().any(|m: &MountInfo| m.mount_point == mount_point) {
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

#[cfg(test)]
mod proc_usage_tests {
    use super::{parse_stat_cpu_ticks, parse_status_memory, proc_usage};

    #[test]
    fn status_memory_in_bytes() {
        let s = "Name:\tstormlb\nVmPeak:\t  9000 kB\nVmSize:\t   8192 kB\nVmRSS:\t   2048 kB\n";
        assert_eq!(parse_status_memory(s), (2048 * 1024, 8192 * 1024));
        assert_eq!(parse_status_memory("Name:\tkthreadd\n"), (0, 0));
    }

    #[test]
    fn stat_cpu_counts_from_the_last_paren() {
        // A command name with a space and a ')' in it.
        let s = "1234 (we ird) x) S 1 1234 1234 0 -1 4194560 100 0 0 0 250 75 0 0 20 0 1 0 999 8388608 512";
        assert_eq!(parse_stat_cpu_ticks(s), Some(325));
        assert_eq!(parse_stat_cpu_ticks("garbage"), None);
    }

    /// stormd#33: a child's own numbers, and its fd count moves when it opens files.
    #[test]
    fn a_child_is_measured_and_its_fds_move() {
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 0.6; exec 7</dev/null 8</dev/null 9</dev/null; sleep 5"])
            .stdin(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let before = proc_usage(pid).expect("child readable");
        std::thread::sleep(std::time::Duration::from_millis(1000));
        let after = proc_usage(pid).expect("child readable");
        let _ = child.kill();
        let _ = child.wait();
        assert!(before.rss_bytes > 0 && before.vm_bytes >= before.rss_bytes, "{before:?}");
        assert!(after.open_fds >= before.open_fds + 3, "fds did not move: {before:?} -> {after:?}");
        assert!(before.cpu_seconds >= 0.0);
        let own = proc_usage(std::process::id()).unwrap();
        assert_ne!(own.rss_bytes, after.rss_bytes, "measured the test process, not the child");
        assert!(proc_usage(u32::MAX - 1).is_none());
    }
}
