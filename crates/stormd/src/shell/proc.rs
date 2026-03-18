use super::ShellOutput;
use crate::api::AppState;
use crate::supervisor::ProcessState;
use chrono::Utc;
use std::sync::Arc;

pub async fn cmd_ps(state: &Arc<AppState>) -> ShellOutput {
    let statuses = state.supervisor.get_all_statuses().await;
    let any_liveness = statuses.iter().any(|s| s.has_liveness);
    let mut out = String::new();
    if any_liveness {
        out.push_str(&format!(
            "\x1b[1m{:<20} {:<12} {:<8} {:<8} {:<12} {:<10} {}\x1b[0m\r\n",
            "PROCESS", "STATE", "PID", "EXIT", "RESTARTS", "LIVENESS", "UPTIME"
        ));
    } else {
        out.push_str(&format!(
            "\x1b[1m{:<20} {:<12} {:<8} {:<8} {:<12} {}\x1b[0m\r\n",
            "PROCESS", "STATE", "PID", "EXIT", "RESTARTS", "UPTIME"
        ));
    }
    for s in &statuses {
        let state_color = match s.state {
            ProcessState::Running => "\x1b[32m",
            ProcessState::Failed => "\x1b[31m",
            ProcessState::Stopped => "\x1b[33m",
            ProcessState::Restarting => "\x1b[36m",
            _ => "\x1b[37m",
        };
        let pid = s
            .pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".into());
        let exit = s
            .exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "-".into());
        let uptime = s
            .uptime_secs
            .map(super::format_duration)
            .unwrap_or_else(|| "-".into());
        if any_liveness {
            let liveness = if s.has_liveness {
                if s.liveness_failures == 0 {
                    "\x1b[32mok\x1b[0m".to_string()
                } else {
                    format!("\x1b[31mfail:{}\x1b[0m", s.liveness_failures)
                }
            } else {
                "-".to_string()
            };
            out.push_str(&format!(
                "{:<20} {}{:<12}\x1b[0m {:<8} {:<8} {:<12} {:<10} {}\r\n",
                s.name,
                state_color,
                format!("{:?}", s.state).to_lowercase(),
                pid,
                exit,
                s.restarts,
                liveness,
                uptime
            ));
        } else {
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
    }
    ShellOutput::text(out)
}

pub async fn cmd_start(state: &Arc<AppState>, name: &str) -> ShellOutput {
    match state.supervisor.start_process(name).await {
        Ok(()) => ShellOutput::text(format!("Started {}\r\n", name)),
        Err(e) => ShellOutput::text(format!("Error: {}\r\n", e)),
    }
}

pub async fn cmd_stop(state: &Arc<AppState>, name: &str) -> ShellOutput {
    match state.supervisor.stop_process(name).await {
        Ok(()) => ShellOutput::text(format!("Stopped {}\r\n", name)),
        Err(e) => ShellOutput::text(format!("Error: {}\r\n", e)),
    }
}

pub async fn cmd_restart(state: &Arc<AppState>, name: &str) -> ShellOutput {
    match state.supervisor.restart_process(name).await {
        Ok(()) => ShellOutput::text(format!("Restarted {}\r\n", name)),
        Err(e) => ShellOutput::text(format!("Error: {}\r\n", e)),
    }
}

pub async fn cmd_cron(state: &Arc<AppState>) -> ShellOutput {
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

pub async fn cmd_status(state: &Arc<AppState>, container_name: &str) -> ShellOutput {
    let statuses = state.supervisor.get_all_statuses().await;
    let failed = state.supervisor.has_failed().await;
    let total = statuses.len();
    let running = statuses
        .iter()
        .filter(|s| s.state == ProcessState::Running)
        .count();
    let restarts: u32 = statuses.iter().map(|s| s.restarts).sum();
    let liveness_count = statuses.iter().filter(|s| s.has_liveness).count();
    let liveness_failing = statuses
        .iter()
        .filter(|s| s.has_liveness && s.liveness_failures > 0)
        .count();

    let status_color = if failed { "\x1b[31m" } else { "\x1b[32m" };
    let status_text = if failed { "FAILED" } else { "HEALTHY" };

    let mut out = String::new();
    out.push_str(&format!("Container:  {}\r\n", container_name));
    out.push_str(&format!(
        "Status:     {}{}\x1b[0m\r\n",
        status_color, status_text
    ));
    out.push_str(&format!("Processes:  {}/{} running\r\n", running, total));
    out.push_str(&format!("Restarts:   {}\r\n", restarts));
    if liveness_count > 0 {
        let liveness_color = if liveness_failing > 0 { "\x1b[31m" } else { "\x1b[32m" };
        out.push_str(&format!(
            "Liveness:   {}{}/{} healthy\x1b[0m\r\n",
            liveness_color,
            liveness_count - liveness_failing,
            liveness_count
        ));
    }
    ShellOutput::text(out)
}

pub async fn cmd_liveness(state: &Arc<AppState>, args: &[&str]) -> ShellOutput {
    use crate::config::ProbeType;

    let statuses = state.supervisor.get_all_statuses().await;

    // Filter to specific process if arg provided
    let filtered: Vec<_> = if let Some(name) = args.first() {
        statuses.into_iter().filter(|s| s.name == *name).collect()
    } else {
        statuses
    };

    if filtered.is_empty() {
        if let Some(name) = args.first() {
            return ShellOutput::text(format!("liveness: process '{}' not found\r\n", name));
        }
        return ShellOutput::text("No processes configured\r\n");
    }

    let any_has_liveness = filtered.iter().any(|s| s.has_liveness);
    if !any_has_liveness {
        return ShellOutput::text("No liveness probes configured\r\n");
    }

    let mut out = String::new();
    for s in &filtered {
        if let Some(ref lc) = s.liveness_config {
            let probe_desc = match &lc.probe {
                ProbeType::Http { url } => format!("http  {}", url),
                ProbeType::Tcp { port } => format!("tcp   127.0.0.1:{}", port),
            };
            let status_str = if s.state != ProcessState::Running {
                "\x1b[33minactive\x1b[0m".to_string()
            } else if s.liveness_failures == 0 {
                "\x1b[32mhealthy\x1b[0m".to_string()
            } else {
                format!(
                    "\x1b[31mfailing ({}/{})\x1b[0m",
                    s.liveness_failures, lc.failure_threshold
                )
            };

            out.push_str(&format!("\x1b[1m{}\x1b[0m\r\n", s.name));
            out.push_str(&format!("  Probe:     {}\r\n", probe_desc));
            out.push_str(&format!("  Status:    {}\r\n", status_str));
            out.push_str(&format!(
                "  Interval:  {}s  Timeout: {}s  Threshold: {}  Delay: {}s\r\n",
                lc.interval_secs, lc.timeout_secs, lc.failure_threshold, lc.initial_delay_secs
            ));
        }
    }
    ShellOutput::text(out)
}

pub fn cmd_uptime(container_name: &str, started_at: chrono::DateTime<Utc>) -> ShellOutput {
    let uptime = (Utc::now() - started_at).num_seconds();
    ShellOutput::text(format!(
        "{} up {}\r\n",
        container_name,
        super::format_duration(uptime)
    ))
}
