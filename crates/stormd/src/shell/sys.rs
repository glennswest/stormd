use super::ShellOutput;
use crate::api::AppState;
use crate::supervisor::ProcessState;
use std::sync::Arc;

pub fn cmd_mount() -> ShellOutput {
    let mounts = crate::stats::StatsCollector::get_mounts();
    if mounts.is_empty() {
        return ShellOutput::text("(no mount info available)\r\n");
    }
    let mut out = String::new();
    for m in &mounts {
        out.push_str(&format!(
            "{} on {} type {} (rw)\r\n",
            m.device, m.mount_point, m.fs_type
        ));
    }
    ShellOutput::text(out)
}

pub fn cmd_df(args: &[&str]) -> ShellOutput {
    let human = args.iter().any(|a| *a == "-h");
    let mounts = crate::stats::StatsCollector::get_mounts();

    if mounts.is_empty() {
        #[cfg(target_os = "linux")]
        {
            // Try /proc/mounts directly
            if let Ok(content) = std::fs::read_to_string("/proc/mounts") {
                let mut out = format!(
                    "{:<24} {:<8} {:<8} {:<8} {:<6} {}\r\n",
                    "Filesystem", "Size", "Used", "Avail", "Use%", "Mounted on"
                );
                for line in content.lines() {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 3 && parts[0].starts_with("/dev/") {
                        out.push_str(&format!(
                            "{:<24} {:<8} {:<8} {:<8} {:<6} {}\r\n",
                            parts[0], "-", "-", "-", "-", parts[1]
                        ));
                    }
                }
                return ShellOutput::text(out);
            }
        }
        return ShellOutput::text("df: no mount info available\r\n");
    }

    let mut out = format!(
        "{:<24} {:>8} {:>8} {:>8} {:>6} {}\r\n",
        "Filesystem", "Size", "Used", "Avail", "Use%", "Mounted on"
    );
    for m in &mounts {
        let (size, used, avail) = if human {
            (
                super::file::format_size_human(m.total_bytes),
                super::file::format_size_human(m.used_bytes),
                super::file::format_size_human(m.avail_bytes),
            )
        } else {
            (
                format!("{}", m.total_bytes / 1024),
                format!("{}", m.used_bytes / 1024),
                format!("{}", m.avail_bytes / 1024),
            )
        };
        out.push_str(&format!(
            "{:<24} {:>8} {:>8} {:>8} {:>5.0}% {}\r\n",
            m.device, size, used, avail, m.use_percent, m.mount_point
        ));
    }
    ShellOutput::text(out)
}

pub fn cmd_free(args: &[&str]) -> ShellOutput {
    let human = args.iter().any(|a| *a == "-h" || *a == "-m");
    #[cfg(target_os = "linux")]
    {
        if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
            let mut total = 0u64;
            let mut free = 0u64;
            let mut available = 0u64;
            let mut buffers = 0u64;
            let mut cached = 0u64;
            for line in content.lines() {
                if let Some(val) = line.strip_prefix("MemTotal:") {
                    total = parse_kb(val);
                } else if let Some(val) = line.strip_prefix("MemFree:") {
                    free = parse_kb(val);
                } else if let Some(val) = line.strip_prefix("MemAvailable:") {
                    available = parse_kb(val);
                } else if let Some(val) = line.strip_prefix("Buffers:") {
                    buffers = parse_kb(val);
                } else if let Some(val) = line.strip_prefix("Cached:") {
                    cached = parse_kb(val);
                }
            }
            let used = total.saturating_sub(free);
            let buff_cache = buffers + cached;

            if human {
                return ShellOutput::text(format!(
                    "              total        used        free      shared  buff/cache   available\r\n\
                     Mem:    {:>10} {:>10} {:>10} {:>10} {:>10} {:>10}\r\n",
                    super::file::format_size_human(total * 1024),
                    super::file::format_size_human(used * 1024),
                    super::file::format_size_human(free * 1024),
                    "0",
                    super::file::format_size_human(buff_cache * 1024),
                    super::file::format_size_human(available * 1024),
                ));
            }
            return ShellOutput::text(format!(
                "              total        used        free   available\r\n\
                 Mem:    {:>10}K {:>10}K {:>10}K {:>10}K\r\n",
                total, used, free, available
            ));
        }
    }
    let _ = human;
    ShellOutput::text("free: memory info not available\r\n")
}

#[cfg(target_os = "linux")]
fn parse_kb(s: &str) -> u64 {
    s.trim()
        .trim_end_matches("kB")
        .trim()
        .parse::<u64>()
        .unwrap_or(0)
}

pub fn cmd_uname(args: &[&str]) -> ShellOutput {
    let show_all = args.iter().any(|a| *a == "-a");
    let show_machine = args.iter().any(|a| *a == "-m");
    let show_release = args.iter().any(|a| *a == "-r");
    let show_sysname = args.iter().any(|a| *a == "-s") || args.is_empty();

    let sysname = if cfg!(target_os = "linux") {
        "Linux"
    } else if cfg!(target_os = "macos") {
        "Darwin"
    } else {
        "Unknown"
    };

    let machine = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "unknown"
    };

    let hostname = std::env::var("HOSTNAME").unwrap_or_else(|_| "stormd".into());

    if show_all {
        // Read kernel release from /proc/version on Linux
        let release = {
            #[cfg(target_os = "linux")]
            {
                std::fs::read_to_string("/proc/version")
                    .ok()
                    .and_then(|v| v.split_whitespace().nth(2).map(|s| s.to_string()))
                    .unwrap_or_else(|| "unknown".into())
            }
            #[cfg(not(target_os = "linux"))]
            {
                "unknown".to_string()
            }
        };
        return ShellOutput::text(format!(
            "{} {} {} {} stormd\r\n",
            sysname, hostname, release, machine
        ));
    }

    if show_machine {
        return ShellOutput::text(format!("{}\r\n", machine));
    }

    if show_release {
        #[cfg(target_os = "linux")]
        {
            if let Ok(ver) = std::fs::read_to_string("/proc/version") {
                if let Some(release) = ver.split_whitespace().nth(2) {
                    return ShellOutput::text(format!("{}\r\n", release));
                }
            }
        }
        return ShellOutput::text("unknown\r\n");
    }

    if show_sysname {
        return ShellOutput::text(format!("{}\r\n", sysname));
    }

    ShellOutput::text(format!("{}\r\n", sysname))
}

pub fn cmd_date() -> ShellOutput {
    let now = chrono::Local::now();
    ShellOutput::text(format!("{}\r\n", now.format("%a %b %e %H:%M:%S %Z %Y")))
}

pub fn cmd_id() -> ShellOutput {
    #[cfg(target_os = "linux")]
    {
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        let user = if uid == 0 { "root" } else { "user" };
        let group = if gid == 0 { "root" } else { "group" };
        ShellOutput::text(format!(
            "uid={}({}) gid={}({})\r\n",
            uid, user, gid, group
        ))
    }
    #[cfg(not(target_os = "linux"))]
    ShellOutput::text("uid=0(root) gid=0(root)\r\n")
}

pub fn cmd_kill(args: &[&str]) -> ShellOutput {
    #[cfg(target_os = "linux")]
    {
        if args.is_empty() {
            return ShellOutput::text("usage: kill [-signal] <pid>\r\n");
        }

        let mut signal = nix::sys::signal::Signal::SIGTERM;
        let mut pid_str = args[0];

        if args[0].starts_with('-') {
            let sig_name = &args[0][1..];
            signal = match sig_name.to_uppercase().as_str() {
                "1" | "HUP" => nix::sys::signal::Signal::SIGHUP,
                "2" | "INT" => nix::sys::signal::Signal::SIGINT,
                "9" | "KILL" => nix::sys::signal::Signal::SIGKILL,
                "15" | "TERM" => nix::sys::signal::Signal::SIGTERM,
                "USR1" | "10" => nix::sys::signal::Signal::SIGUSR1,
                "USR2" | "12" => nix::sys::signal::Signal::SIGUSR2,
                _ => {
                    return ShellOutput::text(format!(
                        "kill: unknown signal '{}'\r\n",
                        sig_name
                    ))
                }
            };
            if args.len() < 2 {
                return ShellOutput::text("usage: kill [-signal] <pid>\r\n");
            }
            pid_str = args[1];
        }

        let pid: i32 = match pid_str.parse() {
            Ok(p) => p,
            Err(_) => {
                return ShellOutput::text(format!(
                    "kill: invalid pid '{}'\r\n",
                    pid_str
                ))
            }
        };

        match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), signal) {
            Ok(()) => ShellOutput::text(""),
            Err(e) => ShellOutput::text(format!("kill: {}\r\n", e)),
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = args;
        ShellOutput::text("kill: not available on this platform\r\n")
    }
}

pub fn cmd_printenv(args: &[&str]) -> ShellOutput {
    if args.is_empty() {
        return cmd_env();
    }
    let var = args[0];
    match std::env::var(var) {
        Ok(val) => ShellOutput::text(format!("{}\r\n", val)),
        Err(_) => ShellOutput::text(""),
    }
}

pub fn cmd_export(args: &[&str]) -> ShellOutput {
    if args.is_empty() {
        return ShellOutput::text("usage: export VAR=value\r\n");
    }
    let expr = args.join(" ");
    if let Some(eq) = expr.find('=') {
        let key = expr[..eq].trim();
        let val = expr[eq + 1..].trim();
        // Remove surrounding quotes
        let val = val
            .trim_start_matches('"')
            .trim_end_matches('"')
            .trim_start_matches('\'')
            .trim_end_matches('\'');
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var(key, val);
        }
        ShellOutput::text("")
    } else {
        ShellOutput::text("usage: export VAR=value\r\n")
    }
}

pub fn cmd_unset(args: &[&str]) -> ShellOutput {
    if args.is_empty() {
        return ShellOutput::text("usage: unset VAR\r\n");
    }
    #[allow(unused_unsafe)]
    unsafe {
        std::env::remove_var(args[0]);
    }
    ShellOutput::text("")
}

pub async fn cmd_sleep(args: &[&str]) -> ShellOutput {
    if args.is_empty() {
        return ShellOutput::text("usage: sleep <seconds>\r\n");
    }
    let secs: f64 = args[0].parse().unwrap_or(1.0);
    let ms = (secs * 1000.0) as u64;
    // Cap at 60 seconds to prevent hung sessions
    let ms = ms.min(60_000);
    tokio::time::sleep(tokio::time::Duration::from_millis(ms)).await;
    ShellOutput::text("")
}

pub fn cmd_echo(args: &[&str]) -> ShellOutput {
    let mut interpret_escapes = false;
    let mut no_newline = false;
    let mut start = 0;

    for (i, arg) in args.iter().enumerate() {
        match *arg {
            "-e" => interpret_escapes = true,
            "-n" => no_newline = true,
            _ => {
                start = i;
                break;
            }
        }
        start = i + 1;
    }

    let text = args[start..].join(" ");
    let mut output = if interpret_escapes {
        text.replace("\\n", "\r\n")
            .replace("\\t", "\t")
            .replace("\\\\", "\\")
    } else {
        text
    };

    if !no_newline {
        output.push_str("\r\n");
    }
    ShellOutput::text(output)
}

pub fn cmd_which(args: &[&str]) -> ShellOutput {
    if args.is_empty() {
        return ShellOutput::text("usage: which <command>\r\n");
    }
    let cmd = args[0];
    if super::is_builtin(cmd) {
        ShellOutput::text(format!("{}: shell built-in command\r\n", cmd))
    } else {
        ShellOutput::text(format!("{}: not found\r\n", cmd))
    }
}

pub fn cmd_type(args: &[&str]) -> ShellOutput {
    if args.is_empty() {
        return ShellOutput::text("usage: type <command>\r\n");
    }
    let cmd = args[0];
    if super::is_builtin(cmd) {
        ShellOutput::text(format!("{} is a shell builtin\r\n", cmd))
    } else {
        ShellOutput::text(format!("-bash: type: {}: not found\r\n", cmd))
    }
}

pub fn cmd_lsof() -> ShellOutput {
    #[cfg(target_os = "linux")]
    {
        let mut out = String::new();
        out.push_str(&format!(
            "\x1b[1m{:<6} {:<40} {}\x1b[0m\r\n",
            "FD", "Target", "Type"
        ));

        if let Ok(entries) = std::fs::read_dir("/proc/self/fd") {
            let mut fds: Vec<(String, String)> = Vec::new();
            for entry in entries.flatten() {
                let fd = entry.file_name().to_string_lossy().to_string();
                let target = std::fs::read_link(entry.path())
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "?".into());
                fds.push((fd, target));
            }
            fds.sort_by(|a, b| {
                a.0.parse::<u32>()
                    .unwrap_or(u32::MAX)
                    .cmp(&b.0.parse::<u32>().unwrap_or(u32::MAX))
            });
            for (fd, target) in &fds {
                let fd_type = if target.starts_with("socket:") {
                    "socket"
                } else if target.starts_with("pipe:") {
                    "pipe"
                } else if target.starts_with("anon_inode:") {
                    "anon_inode"
                } else if target.starts_with("/dev/") {
                    "device"
                } else {
                    "file"
                };
                out.push_str(&format!("{:<6} {:<40} {}\r\n", fd, target, fd_type));
            }
        }
        ShellOutput::text(out)
    }

    #[cfg(not(target_os = "linux"))]
    ShellOutput::text("lsof: not available (Linux only)\r\n")
}

pub async fn cmd_systemctl(state: &Arc<AppState>, args: &[&str]) -> ShellOutput {
    let subcmd = args.first().copied().unwrap_or("list-units");
    let name = args.get(1).copied();

    match subcmd {
        "status" => {
            if let Some(name) = name {
                match state.supervisor.get_status(name).await {
                    Ok(s) => {
                        let state_color = match s.state {
                            ProcessState::Running => "\x1b[32m",
                            ProcessState::Failed => "\x1b[31m",
                            ProcessState::Stopped => "\x1b[33m",
                            _ => "\x1b[37m",
                        };
                        let mut out = format!(
                            "\x1b[1m{}\x1b[0m - {}\r\n   Loaded: loaded (stormd)\r\n   Active: {}{:?}\x1b[0m\r\n",
                            name, name, state_color, s.state
                        );
                        if let Some(pid) = s.pid {
                            out.push_str(&format!("  Main PID: {}\r\n", pid));
                        }
                        if s.restarts > 0 {
                            out.push_str(&format!("  Restarts: {}\r\n", s.restarts));
                        }
                        if s.has_liveness {
                            let liveness_str = if s.liveness_failures == 0 {
                                "\x1b[32mhealthy\x1b[0m".to_string()
                            } else {
                                format!("\x1b[31m{} consecutive failure(s)\x1b[0m", s.liveness_failures)
                            };
                            out.push_str(&format!("  Liveness: {}\r\n", liveness_str));
                        }
                        ShellOutput::text(out)
                    }
                    Err(e) => ShellOutput::text(format!(
                        "Unit {} not found: {}\r\n",
                        name, e
                    )),
                }
            } else {
                super::proc::cmd_ps(state).await
            }
        }
        "start" => match name {
            Some(n) => super::proc::cmd_start(state, n).await,
            None => ShellOutput::text("usage: systemctl start <unit>\r\n"),
        },
        "stop" => match name {
            Some(n) => super::proc::cmd_stop(state, n).await,
            None => ShellOutput::text("usage: systemctl stop <unit>\r\n"),
        },
        "restart" => match name {
            Some(n) => super::proc::cmd_restart(state, n).await,
            None => ShellOutput::text("usage: systemctl restart <unit>\r\n"),
        },
        "list-units" => super::proc::cmd_ps(state).await,
        "is-active" => match name {
            Some(n) => match state.supervisor.get_status(n).await {
                Ok(s) => {
                    let active = s.state == ProcessState::Running;
                    ShellOutput::text(format!(
                        "{}\r\n",
                        if active { "active" } else { "inactive" }
                    ))
                }
                Err(_) => ShellOutput::text("inactive\r\n"),
            },
            None => ShellOutput::text("usage: systemctl is-active <unit>\r\n"),
        },
        "is-failed" => match name {
            Some(n) => match state.supervisor.get_status(n).await {
                Ok(s) => {
                    let failed = s.state == ProcessState::Failed;
                    ShellOutput::text(format!(
                        "{}\r\n",
                        if failed { "failed" } else { "active" }
                    ))
                }
                Err(_) => ShellOutput::text("unknown\r\n"),
            },
            None => ShellOutput::text("usage: systemctl is-failed <unit>\r\n"),
        },
        "enable" | "disable" => ShellOutput::text(
            "systemctl enable/disable: not supported in stormd (processes are config-driven)\r\n",
        ),
        _ => ShellOutput::text(format!(
            "systemctl: unknown command '{}'\r\nUsage: systemctl {{status|start|stop|restart|list-units|is-active|is-failed}} [unit]\r\n",
            subcmd
        )),
    }
}

pub fn cmd_env() -> ShellOutput {
    let mut out = String::new();
    for (k, v) in std::env::vars() {
        out.push_str(&format!("{}={}\r\n", k, v));
    }
    ShellOutput::text(out)
}

pub fn cmd_whoami() -> ShellOutput {
    ShellOutput::text("root\r\n")
}
