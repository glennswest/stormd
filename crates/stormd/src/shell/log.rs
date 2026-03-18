use super::ShellOutput;
use crate::api::AppState;
use std::sync::Arc;

pub async fn cmd_logs(state: &Arc<AppState>, args: &[&str]) -> ShellOutput {
    let mut follow = false;
    let mut process = None;
    let mut tail = Some(50usize);

    for arg in args {
        match *arg {
            "-f" => follow = true,
            "-n" => {} // next arg is the count, handled below
            _ if arg.parse::<usize>().is_ok() && args.iter().any(|a| *a == "-n") => {
                tail = arg.parse().ok();
            }
            _ if !arg.starts_with('-') => process = Some(arg.to_string()),
            _ => {}
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

    let query = stormlog::types::LogQuery {
        process,
        tail,
        ..Default::default()
    };

    match state.stormlog.query_logs(&query).await {
        Ok(entries) => {
            if entries.is_empty() {
                ShellOutput::text("(no stored logs — MinIO may not be configured)\r\n")
            } else {
                let mut out = String::new();
                for entry in entries {
                    let sev_color = match entry.severity {
                        stormlog::types::Severity::Error
                        | stormlog::types::Severity::Critical => "\x1b[31m",
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

pub async fn cmd_grep_logs(state: &Arc<AppState>, pattern: &str) -> ShellOutput {
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

pub async fn cmd_dmesg(state: &Arc<AppState>, args: &[&str]) -> ShellOutput {
    let mut follow = false;
    let mut tail = Some(50usize);

    for arg in args {
        match *arg {
            "-f" | "--follow" => follow = true,
            "-n" => {}
            _ if arg.parse::<usize>().is_ok() => {
                tail = arg.parse().ok();
            }
            _ => {}
        }
    }

    if follow {
        return ShellOutput {
            text: "Following system logs... (Ctrl-C to stop)\r\n".to_string(),
            exit: false,
            attach: None,
            follow: true,
            follow_process: None,
        };
    }

    let query = stormlog::types::LogQuery {
        tail,
        ..Default::default()
    };

    match state.stormlog.query_logs(&query).await {
        Ok(entries) => {
            let mut out = String::new();
            for entry in entries {
                let sev_color = match entry.severity {
                    stormlog::types::Severity::Error
                    | stormlog::types::Severity::Critical => "\x1b[31m",
                    stormlog::types::Severity::Warning => "\x1b[33m",
                    stormlog::types::Severity::Debug => "\x1b[90m",
                    _ => "\x1b[0m",
                };
                out.push_str(&format!(
                    "[{}{}{}] {} \x1b[36m{}\x1b[0m {}\r\n",
                    sev_color,
                    format!("{:?}", entry.severity).to_uppercase(),
                    "\x1b[0m",
                    entry.timestamp.format("%H:%M:%S%.3f"),
                    entry.process,
                    entry.line
                ));
            }
            if out.is_empty() {
                out = "(no log entries)\r\n".to_string();
            }
            ShellOutput::text(out)
        }
        Err(_) => ShellOutput::text("(log system unavailable)\r\n"),
    }
}
