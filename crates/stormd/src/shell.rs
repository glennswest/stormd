use crate::api::AppState;
use crate::supervisor::ProcessState;
use chrono::Utc;
use std::sync::Arc;

/// Shell command result — text to send back to the terminal.
pub struct ShellOutput {
    pub text: String,
    /// If true, the shell session should end.
    pub exit: bool,
    /// If set, attach to this process's terminal (interactive mode).
    pub attach: Option<String>,
    /// If true, enter follow/tail mode.
    pub follow: bool,
    pub follow_process: Option<String>,
}

impl ShellOutput {
    fn text(s: impl Into<String>) -> Self {
        Self {
            text: s.into(),
            exit: false,
            attach: None,
            follow: false,
            follow_process: None,
        }
    }

    fn exit() -> Self {
        Self {
            text: "logout\r\n".to_string(),
            exit: true,
            attach: None,
            follow: false,
            follow_process: None,
        }
    }
}

/// Execute a shell command and return output text.
pub async fn execute_command(
    line: &str,
    state: &Arc<AppState>,
    container_name: &str,
    started_at: chrono::DateTime<Utc>,
) -> ShellOutput {
    let line = line.trim();
    if line.is_empty() {
        return ShellOutput::text("");
    }

    // Handle piping: `logs | grep pattern`
    if let Some(pipe_pos) = line.find(" | grep ") {
        let left = &line[..pipe_pos];
        let pattern = line[pipe_pos + 8..].trim();
        let output = execute_single(left, state, container_name, started_at).await;
        if output.exit {
            return output;
        }
        let filtered: Vec<&str> = output
            .text
            .lines()
            .filter(|l| l.contains(pattern))
            .collect();
        return ShellOutput::text(filtered.join("\r\n") + "\r\n");
    }

    execute_single(line, state, container_name, started_at).await
}

async fn execute_single(
    line: &str,
    state: &Arc<AppState>,
    container_name: &str,
    started_at: chrono::DateTime<Utc>,
) -> ShellOutput {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.is_empty() {
        return ShellOutput::text("");
    }

    let cmd = parts[0];
    let args = &parts[1..];

    match cmd {
        "ps" => cmd_ps(state).await,
        "top" => cmd_ps(state).await, // Static view (true top would need a refresh loop)
        "start" => {
            if args.is_empty() {
                ShellOutput::text("usage: start <process>\r\n")
            } else {
                cmd_start(state, args[0]).await
            }
        }
        "stop" => {
            if args.is_empty() {
                ShellOutput::text("usage: stop <process>\r\n")
            } else {
                cmd_stop(state, args[0]).await
            }
        }
        "restart" => {
            if args.is_empty() {
                ShellOutput::text("usage: restart <process>\r\n")
            } else {
                cmd_restart(state, args[0]).await
            }
        }
        "attach" => {
            if args.is_empty() {
                ShellOutput::text("usage: attach <process>\r\n")
            } else {
                ShellOutput {
                    text: format!("Attaching to {}... (Ctrl-C to detach)\r\n", args[0]),
                    exit: false,
                    attach: Some(args[0].to_string()),
                    follow: false,
                    follow_process: None,
                }
            }
        }
        "logs" => cmd_logs(state, args).await,
        "grep" => {
            if args.is_empty() {
                ShellOutput::text("usage: grep <pattern>\r\n")
            } else {
                cmd_grep(state, args[0]).await
            }
        }
        "cron" => cmd_cron(state).await,
        "status" => cmd_status(state, container_name).await,
        "uptime" => cmd_uptime(container_name, started_at),
        "env" => cmd_env(),
        "whoami" => ShellOutput::text("root\r\n"),
        "hostname" => ShellOutput::text(format!("{}\r\n", container_name)),
        "df" => cmd_df().await,
        "free" => cmd_free(),
        "help" | "?" => cmd_help(),
        "exit" | "logout" | "quit" => ShellOutput::exit(),
        "clear" => ShellOutput::text("\x1b[2J\x1b[H"),
        "cat" | "ls" => ShellOutput::text(format!(
            "Note: cat/ls operate on MinIO log objects (not yet connected)\r\n"
        )),
        _ => ShellOutput::text(format!("{}: command not found\r\n", cmd)),
    }
}

async fn cmd_ps(state: &Arc<AppState>) -> ShellOutput {
    let statuses = state.supervisor.get_all_statuses().await;
    let mut out = String::new();
    out.push_str(&format!(
        "\x1b[1m{:<20} {:<12} {:<8} {:<8} {:<12} {}\x1b[0m\r\n",
        "PROCESS", "STATE", "PID", "EXIT", "RESTARTS", "UPTIME"
    ));
    for s in &statuses {
        let state_color = match s.state {
            ProcessState::Running => "\x1b[32m",   // green
            ProcessState::Failed => "\x1b[31m",     // red
            ProcessState::Stopped => "\x1b[33m",    // yellow
            ProcessState::Restarting => "\x1b[36m", // cyan
            _ => "\x1b[37m",                        // white
        };
        let pid = s.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".to_string());
        let exit = s
            .exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "-".to_string());
        let uptime = s
            .uptime_secs
            .map(|u| format_duration(u))
            .unwrap_or_else(|| "-".to_string());
        out.push_str(&format!(
            "{:<20} {}{:<12}\x1b[0m {:<8} {:<8} {:<12} {}\r\n",
            s.name,
            state_color,
            format!("{:?}", s.state).to_lowercase(),
            pid,
            exit,
            s.restarts,
            uptime
        ));
    }
    ShellOutput::text(out)
}

async fn cmd_start(state: &Arc<AppState>, name: &str) -> ShellOutput {
    match state.supervisor.start_process(name).await {
        Ok(()) => ShellOutput::text(format!("Started {}\r\n", name)),
        Err(e) => ShellOutput::text(format!("Error: {}\r\n", e)),
    }
}

async fn cmd_stop(state: &Arc<AppState>, name: &str) -> ShellOutput {
    match state.supervisor.stop_process(name).await {
        Ok(()) => ShellOutput::text(format!("Stopped {}\r\n", name)),
        Err(e) => ShellOutput::text(format!("Error: {}\r\n", e)),
    }
}

async fn cmd_restart(state: &Arc<AppState>, name: &str) -> ShellOutput {
    match state.supervisor.restart_process(name).await {
        Ok(()) => ShellOutput::text(format!("Restarted {}\r\n", name)),
        Err(e) => ShellOutput::text(format!("Error: {}\r\n", e)),
    }
}

async fn cmd_logs(state: &Arc<AppState>, args: &[&str]) -> ShellOutput {
    let mut follow = false;
    let mut process = None;

    for arg in args {
        if *arg == "-f" {
            follow = true;
        } else {
            process = Some(arg.to_string());
        }
    }

    if follow {
        return ShellOutput {
            text: format!(
                "Following logs{}... (Ctrl-C to stop)\r\n",
                process
                    .as_ref()
                    .map(|p| format!(" for {}", p))
                    .unwrap_or_default()
            ),
            exit: false,
            attach: None,
            follow: true,
            follow_process: process,
        };
    }

    // Read recent log files
    let query = stormlog::types::LogQuery {
        process,
        tail: Some(50),
        ..Default::default()
    };

    match state.stormlog.query_logs(&query).await {
        Ok(entries) => {
            if entries.is_empty() {
                // Fallback to file-based logs
                ShellOutput::text("(no stored logs — MinIO may not be configured)\r\n")
            } else {
                let mut out = String::new();
                for entry in entries {
                    let sev_color = match entry.severity {
                        stormlog::types::Severity::Error | stormlog::types::Severity::Critical => {
                            "\x1b[31m"
                        }
                        stormlog::types::Severity::Warning => "\x1b[33m",
                        stormlog::types::Severity::Debug => "\x1b[90m",
                        _ => "\x1b[0m",
                    };
                    out.push_str(&format!(
                        "{} \x1b[36m{}\x1b[0m [{}{}{}] {}\r\n",
                        entry.timestamp.format("%H:%M:%S"),
                        entry.process,
                        sev_color,
                        entry.stream,
                        "\x1b[0m",
                        entry.line
                    ));
                }
                ShellOutput::text(out)
            }
        }
        Err(_) => ShellOutput::text("(no stored logs available)\r\n"),
    }
}

async fn cmd_grep(state: &Arc<AppState>, pattern: &str) -> ShellOutput {
    let query = stormlog::types::LogQuery {
        search: Some(pattern.to_string()),
        tail: Some(50),
        ..Default::default()
    };

    match state.stormlog.query_logs(&query).await {
        Ok(entries) => {
            let mut out = String::new();
            for entry in entries {
                out.push_str(&format!(
                    "{} [{}] {}\r\n",
                    entry.process, entry.stream, entry.line
                ));
            }
            if out.is_empty() {
                out = "(no matches)\r\n".to_string();
            }
            ShellOutput::text(out)
        }
        Err(_) => ShellOutput::text("(search unavailable)\r\n"),
    }
}

async fn cmd_cron(state: &Arc<AppState>) -> ShellOutput {
    let jobs = state.cron_scheduler.get_status().await;
    if jobs.is_empty() {
        return ShellOutput::text("No cron jobs configured\r\n");
    }
    let mut out = String::new();
    out.push_str(&format!(
        "\x1b[1m{:<20} {:<25} {:<8} {:<8} {}\x1b[0m\r\n",
        "JOB", "SCHEDULE", "RUNS", "FAILS", "NEXT RUN"
    ));
    for j in &jobs {
        out.push_str(&format!(
            "{:<20} {:<25} {:<8} {:<8} {}\r\n",
            j.name,
            j.schedule,
            j.run_count,
            j.fail_count,
            j.next_run.as_deref().unwrap_or("-")
        ));
    }
    ShellOutput::text(out)
}

async fn cmd_status(state: &Arc<AppState>, container_name: &str) -> ShellOutput {
    let statuses = state.supervisor.get_all_statuses().await;
    let failed = state.supervisor.has_failed().await;
    let total = statuses.len();
    let running = statuses
        .iter()
        .filter(|s| s.state == ProcessState::Running)
        .count();
    let restarts: u32 = statuses.iter().map(|s| s.restarts).sum();

    let status_color = if failed { "\x1b[31m" } else { "\x1b[32m" };
    let status_text = if failed { "FAILED" } else { "HEALTHY" };

    let mut out = String::new();
    out.push_str(&format!(
        "Container:  {}\r\n",
        container_name
    ));
    out.push_str(&format!(
        "Status:     {}{}\x1b[0m\r\n",
        status_color, status_text
    ));
    out.push_str(&format!(
        "Processes:  {}/{} running\r\n",
        running, total
    ));
    out.push_str(&format!(
        "Restarts:   {}\r\n",
        restarts
    ));
    ShellOutput::text(out)
}

fn cmd_uptime(container_name: &str, started_at: chrono::DateTime<Utc>) -> ShellOutput {
    let uptime = (Utc::now() - started_at).num_seconds();
    ShellOutput::text(format!(
        "{} up {}\r\n",
        container_name,
        format_duration(uptime)
    ))
}

fn cmd_env() -> ShellOutput {
    let mut out = String::new();
    for (k, v) in std::env::vars() {
        out.push_str(&format!("{}={}\r\n", k, v));
    }
    ShellOutput::text(out)
}

async fn cmd_df() -> ShellOutput {
    // Simple df — read /proc/mounts or just show a placeholder
    ShellOutput::text("Filesystem      Size  Used Avail Use% Mounted on\r\n(df not available in scratch container)\r\n")
}

fn cmd_free() -> ShellOutput {
    #[cfg(target_os = "linux")]
    {
        if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
            let mut total = 0u64;
            let mut free = 0u64;
            let mut available = 0u64;
            for line in content.lines() {
                if let Some(val) = line.strip_prefix("MemTotal:") {
                    total = parse_kb(val);
                } else if let Some(val) = line.strip_prefix("MemFree:") {
                    free = parse_kb(val);
                } else if let Some(val) = line.strip_prefix("MemAvailable:") {
                    available = parse_kb(val);
                }
            }
            let used = total.saturating_sub(free);
            return ShellOutput::text(format!(
                "              total        used        free   available\r\nMem:    {:>10}K {:>10}K {:>10}K {:>10}K\r\n",
                total, used, free, available
            ));
        }
    }
    ShellOutput::text("free: memory info not available\r\n")
}

#[cfg(target_os = "linux")]
fn parse_kb(s: &str) -> u64 {
    s.trim().trim_end_matches("kB").trim().parse().unwrap_or(0)
}

fn cmd_help() -> ShellOutput {
    ShellOutput::text(
        "\x1b[1mstormd shell\x1b[0m — built-in management console\r\n\
         \r\n\
         \x1b[1mProcess Management:\x1b[0m\r\n\
         \x20 ps                  List supervised processes\r\n\
         \x20 start <name>        Start a process\r\n\
         \x20 stop <name>         Stop a process\r\n\
         \x20 restart <name>      Restart a process\r\n\
         \x20 attach <name>       Attach to process terminal (Ctrl-C detach)\r\n\
         \r\n\
         \x1b[1mLogs:\x1b[0m\r\n\
         \x20 logs [name]         Show recent logs\r\n\
         \x20 logs -f [name]      Follow logs realtime\r\n\
         \x20 grep <pattern>      Search logs\r\n\
         \r\n\
         \x1b[1mSystem:\x1b[0m\r\n\
         \x20 status              Full system status\r\n\
         \x20 uptime              Container uptime\r\n\
         \x20 cron                List cron jobs\r\n\
         \x20 env                 Environment variables\r\n\
         \x20 whoami              Current user\r\n\
         \x20 hostname            Container name\r\n\
         \x20 df                  Storage usage\r\n\
         \x20 free                Memory info\r\n\
         \x20 clear               Clear screen\r\n\
         \x20 help                This help message\r\n\
         \x20 exit                Close session\r\n\
         \r\n\
         \x1b[1mPiping:\x1b[0m\r\n\
         \x20 logs | grep error   Filter output through grep\r\n",
    )
}

fn format_duration(secs: i64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let secs = secs % 60;
    if days > 0 {
        format!("{}d {}h {}m", days, hours, mins)
    } else if hours > 0 {
        format!("{}h {}m {}s", hours, mins, secs)
    } else if mins > 0 {
        format!("{}m {}s", mins, secs)
    } else {
        format!("{}s", secs)
    }
}

/// Tab completion for commands and process names.
pub async fn complete(partial: &str, state: &Arc<AppState>) -> Vec<String> {
    let commands = [
        "ps", "top", "start", "stop", "restart", "attach", "logs", "grep", "cat", "ls",
        "cron", "status", "uptime", "env", "whoami", "hostname", "df", "free", "help",
        "exit", "logout", "clear",
    ];

    let parts: Vec<&str> = partial.split_whitespace().collect();

    if parts.len() <= 1 {
        // Complete command
        let prefix = parts.first().copied().unwrap_or("");
        return commands
            .iter()
            .filter(|c| c.starts_with(prefix))
            .map(|c| c.to_string())
            .collect();
    }

    // Complete process names for commands that take a process argument
    let cmd = parts[0];
    if matches!(cmd, "start" | "stop" | "restart" | "attach" | "logs") {
        let prefix = parts.last().copied().unwrap_or("");
        let names = state.supervisor.process_names().await;
        return names
            .into_iter()
            .filter(|n| n.starts_with(prefix))
            .collect();
    }

    Vec::new()
}
