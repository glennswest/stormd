use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct DebugInfo {
    pub pid: u32,
    pub env: Vec<EnvVar>,
    pub open_fds: Option<u32>,
    pub cwd: String,
    pub exe: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct SignalRequest {
    pub signal: String,
}

pub fn collect_debug_info() -> DebugInfo {
    let env: Vec<EnvVar> = std::env::vars()
        .map(|(k, v)| EnvVar { key: k, value: v })
        .collect();

    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let open_fds = count_open_fds();

    DebugInfo {
        pid: std::process::id(),
        env,
        open_fds,
        cwd,
        exe,
    }
}

fn count_open_fds() -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_dir("/proc/self/fd")
            .ok()
            .map(|entries| entries.count() as u32)
    }

    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Send a signal to a process by PID (Linux only).
#[cfg(target_os = "linux")]
pub fn send_signal(pid: u32, signal_name: &str) -> anyhow::Result<()> {
    use nix::sys::signal::{self, Signal};
    use nix::unistd::Pid;
    use std::str::FromStr;

    let sig = Signal::from_str(&signal_name.to_uppercase())
        .or_else(|_| Signal::from_str(&format!("SIG{}", signal_name.to_uppercase())))
        .map_err(|_| anyhow::anyhow!("unknown signal: {}", signal_name))?;

    signal::kill(Pid::from_raw(pid as i32), sig)?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn send_signal(_pid: u32, _signal_name: &str) -> anyhow::Result<()> {
    anyhow::bail!("signal sending only supported on Linux")
}
