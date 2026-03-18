mod file;
mod log;
mod net;
mod proc;
mod sys;
mod text;

use crate::api::AppState;
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

/// Execute a shell command line with piping and redirection support.
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

    // Split on pipes: `cmd1 | cmd2 | cmd3`
    let segments: Vec<&str> = line.split(" | ").collect();

    // Check last segment for output redirection (> or >>)
    let last = segments[segments.len() - 1];
    let (last_cmd, redirect) = parse_redirect(last);

    // Build final segment list
    let mut final_segments: Vec<String> = segments[..segments.len() - 1]
        .iter()
        .map(|s| s.to_string())
        .collect();
    final_segments.push(last_cmd);

    // Execute first command
    let mut output =
        execute_single(&final_segments[0], state, container_name, started_at, None).await;

    // Chain through remaining commands
    for seg in &final_segments[1..] {
        if output.exit || output.follow || output.attach.is_some() {
            return output;
        }
        let piped = output.text.clone();
        output =
            execute_single(seg.trim(), state, container_name, started_at, Some(&piped)).await;
    }

    // Handle redirect
    if let Some((path, append)) = redirect {
        if output.exit {
            return output;
        }
        let content = output.text.replace("\r\n", "\n");
        let result = if append {
            use tokio::io::AsyncWriteExt;
            match tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .await
            {
                Ok(mut f) => f.write_all(content.as_bytes()).await,
                Err(e) => Err(e),
            }
        } else {
            tokio::fs::write(&path, content.as_bytes()).await
        };
        return match result {
            Ok(_) => ShellOutput::text(""),
            Err(e) => ShellOutput::text(format!("redirect: {}\r\n", e)),
        };
    }

    output
}

fn parse_redirect(segment: &str) -> (String, Option<(String, bool)>) {
    // Check for >> first (append)
    if let Some(pos) = segment.find(" >> ") {
        let cmd = segment[..pos].trim().to_string();
        let path = segment[pos + 4..].trim().to_string();
        return (cmd, Some((path, true)));
    }
    // Check for > (overwrite)
    if let Some(pos) = segment.find(" > ") {
        let cmd = segment[..pos].trim().to_string();
        let path = segment[pos + 3..].trim().to_string();
        return (cmd, Some((path, false)));
    }
    (segment.to_string(), None)
}

pub(crate) async fn execute_single(
    line: &str,
    state: &Arc<AppState>,
    container_name: &str,
    started_at: chrono::DateTime<Utc>,
    piped_input: Option<&str>,
) -> ShellOutput {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.is_empty() {
        return ShellOutput::text("");
    }

    let cmd = parts[0];
    let args = &parts[1..];

    match cmd {
        // Process management
        "ps" | "top" => proc::cmd_ps(state).await,
        "start" => {
            if args.is_empty() {
                ShellOutput::text("usage: start <process>\r\n")
            } else {
                proc::cmd_start(state, args[0]).await
            }
        }
        "stop" => {
            if args.is_empty() {
                ShellOutput::text("usage: stop <process>\r\n")
            } else {
                proc::cmd_stop(state, args[0]).await
            }
        }
        "restart" => {
            if args.is_empty() {
                ShellOutput::text("usage: restart <process>\r\n")
            } else {
                proc::cmd_restart(state, args[0]).await
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
        "cron" => proc::cmd_cron(state).await,
        "liveness" => proc::cmd_liveness(state, args).await,
        "status" => proc::cmd_status(state, container_name).await,
        "uptime" => proc::cmd_uptime(container_name, started_at),

        // Logs
        "logs" => log::cmd_logs(state, args).await,
        "dmesg" => log::cmd_dmesg(state, args).await,
        "grep" => {
            // If piped or file argument present, use text grep; otherwise log search
            if piped_input.is_some() || args.len() >= 2 {
                text::cmd_grep(args, piped_input).await
            } else if !args.is_empty() {
                log::cmd_grep_logs(state, args[0]).await
            } else {
                ShellOutput::text("usage: grep <pattern> [file]\r\n")
            }
        }

        // File operations
        "ls" | "dir" => file::cmd_ls(args).await,
        "cat" => file::cmd_cat(args, piped_input).await,
        "head" => file::cmd_head(args, piped_input).await,
        "tail" => file::cmd_tail(args, piped_input).await,
        "cp" => file::cmd_cp(args).await,
        "mv" => file::cmd_mv(args).await,
        "rm" => file::cmd_rm(args).await,
        "mkdir" => file::cmd_mkdir(args).await,
        "touch" => file::cmd_touch(args).await,
        "chmod" => file::cmd_chmod(args).await,
        "chown" => file::cmd_chown(args).await,
        "find" => file::cmd_find(args).await,
        "ln" => file::cmd_ln(args).await,
        "stat" => file::cmd_stat(args).await,
        "pwd" => file::cmd_pwd(),
        "wc" => file::cmd_wc(args, piped_input).await,
        "du" => file::cmd_du(args).await,
        "readlink" => file::cmd_readlink(args).await,
        "file" => file::cmd_file_type(args).await,
        "sha256sum" => file::cmd_sha256sum(args, piped_input).await,
        "tee" => file::cmd_tee(args, piped_input).await,

        // Network
        "ifconfig" => net::cmd_ifconfig(args).await,
        "ip" => net::cmd_ip(args).await,
        "ping" => net::cmd_ping(args).await,
        "curl" | "wget" => net::cmd_curl(args).await,
        "netstat" => net::cmd_netstat(args).await,
        "ss" => net::cmd_ss(args).await,
        "nslookup" | "dig" => net::cmd_nslookup(args).await,
        "hostname" => net::cmd_hostname(container_name, args),
        "route" => net::cmd_route(args).await,

        // System
        "mount" => sys::cmd_mount(),
        "df" => sys::cmd_df(args),
        "free" => sys::cmd_free(args),
        "uname" => sys::cmd_uname(args),
        "date" => sys::cmd_date(),
        "id" => sys::cmd_id(),
        "kill" => sys::cmd_kill(args),
        "printenv" => sys::cmd_printenv(args),
        "export" => sys::cmd_export(args),
        "unset" => sys::cmd_unset(args),
        "sleep" => sys::cmd_sleep(args).await,
        "echo" => sys::cmd_echo(args),
        "env" => sys::cmd_env(),
        "whoami" => sys::cmd_whoami(),
        "which" => sys::cmd_which(args),
        "type" => sys::cmd_type(args),
        "lsof" => sys::cmd_lsof(),
        "systemctl" => sys::cmd_systemctl(state, args).await,
        "true" => ShellOutput::text(""),
        "false" => ShellOutput::text(""),

        // Text processing
        "sort" => text::cmd_sort(args, piped_input).await,
        "uniq" => text::cmd_uniq(args, piped_input).await,
        "cut" => text::cmd_cut(args, piped_input).await,
        "tr" => text::cmd_tr(args, piped_input).await,
        "sed" => text::cmd_sed(args, piped_input).await,
        "rev" => text::cmd_rev(piped_input),
        "base64" => text::cmd_base64(args, piped_input),
        "xxd" => text::cmd_xxd(piped_input),
        "xargs" => text::cmd_xargs(args, piped_input, state, container_name, started_at).await,
        "md5sum" => file::cmd_sha256sum(args, piped_input).await, // alias to sha256sum

        // Shell builtins
        "help" | "?" => cmd_help(),
        "exit" | "logout" | "quit" => ShellOutput::exit(),
        "clear" => ShellOutput::text("\x1b[2J\x1b[H"),

        _ => ShellOutput::text(format!("{}: command not found\r\n", cmd)),
    }
}

/// Check if a command name is a builtin.
pub(crate) fn is_builtin(cmd: &str) -> bool {
    ALL_COMMANDS.contains(&cmd)
}

const ALL_COMMANDS: &[&str] = &[
    "ps", "top", "start", "stop", "restart", "attach", "logs", "grep", "dmesg", "cron", "liveness", "status",
    "uptime", "ls", "dir", "cat", "head", "tail", "cp", "mv", "rm", "mkdir", "touch", "chmod",
    "chown", "find", "ln", "stat", "pwd", "wc", "du", "readlink", "file", "sha256sum", "tee",
    "ifconfig", "ip", "ping", "curl", "wget", "netstat", "ss", "nslookup", "dig", "hostname",
    "route", "mount", "df", "free", "uname", "date", "id", "kill", "printenv", "export", "unset",
    "sleep", "echo", "env", "whoami", "which", "type", "lsof", "systemctl", "true", "false",
    "sort", "uniq", "cut", "tr", "sed", "rev", "base64", "xxd", "xargs", "help", "exit",
    "logout", "quit", "clear",
];

fn cmd_help() -> ShellOutput {
    ShellOutput::text(
        "\x1b[1mstormd shell\x1b[0m — busybox-style management console\r\n\
         \r\n\
         \x1b[1mProcess Management:\x1b[0m\r\n\
         \x20 ps / top              List supervised processes\r\n\
         \x20 start <name>          Start a process\r\n\
         \x20 stop <name>           Stop a process\r\n\
         \x20 restart <name>        Restart a process\r\n\
         \x20 attach <name>         Attach to process terminal\r\n\
         \x20 liveness [name]       Show liveness probe config/status\r\n\
         \x20 systemctl <cmd>       Systemd-style process control\r\n\
         \r\n\
         \x1b[1mLogs:\x1b[0m\r\n\
         \x20 logs [-f] [name]      Show/follow logs\r\n\
         \x20 grep <pattern> [file] Search logs or files\r\n\
         \x20 dmesg [-f]            System log (all processes)\r\n\
         \r\n\
         \x1b[1mFile Operations:\x1b[0m\r\n\
         \x20 ls [-la] [path]       List directory\r\n\
         \x20 cat <file>            Show file contents\r\n\
         \x20 head/tail [-n N] <f>  First/last N lines\r\n\
         \x20 cp/mv/rm/mkdir/touch  File management\r\n\
         \x20 chmod/chown           Permissions\r\n\
         \x20 find <path> -name '*' Find files\r\n\
         \x20 stat/file/readlink    File info\r\n\
         \x20 wc [-lwc] <file>      Word/line/char count\r\n\
         \x20 du [-sh] [path]       Disk usage\r\n\
         \x20 sha256sum <file>      Hash file\r\n\
         \r\n\
         \x1b[1mNetwork:\x1b[0m\r\n\
         \x20 ifconfig / ip addr    Network interfaces\r\n\
         \x20 ping [-c N] <host>    Test connectivity\r\n\
         \x20 curl/wget <url>       HTTP requests\r\n\
         \x20 netstat / ss          Socket connections\r\n\
         \x20 nslookup <host>       DNS lookup\r\n\
         \x20 route                 Routing table\r\n\
         \r\n\
         \x1b[1mSystem:\x1b[0m\r\n\
         \x20 mount / df [-h]       Mounts and disk space\r\n\
         \x20 free [-h]             Memory info\r\n\
         \x20 uname [-a]            System info\r\n\
         \x20 date / uptime / id    Time, uptime, user\r\n\
         \x20 kill [-sig] <pid>     Send signal\r\n\
         \x20 env / printenv        Environment\r\n\
         \x20 export / unset        Set/clear vars\r\n\
         \x20 lsof                  Open file descriptors\r\n\
         \x20 cron                  Cron job list\r\n\
         \r\n\
         \x1b[1mText Processing:\x1b[0m\r\n\
         \x20 sort [-rnu] / uniq    Sort/deduplicate\r\n\
         \x20 cut -d<d> -f<n>       Extract fields\r\n\
         \x20 sed 's/pat/rep/'      Substitution\r\n\
         \x20 tr <from> <to>        Character translation\r\n\
         \x20 base64 [-d] / xxd     Encode/decode/hexdump\r\n\
         \x20 rev / tee / xargs     Transform piped data\r\n\
         \r\n\
         \x1b[1mPiping & Redirection:\x1b[0m\r\n\
         \x20 cmd1 | cmd2 | cmd3    Pipe output between commands\r\n\
         \x20 cmd > file            Write output to file\r\n\
         \x20 cmd >> file           Append output to file\r\n\
         \r\n\
         \x20 clear / exit          Clear screen / close session\r\n",
    )
}

/// Commands that can run standalone (no AppState / supervisor).
pub const STANDALONE_COMMANDS: &[&str] = &[
    // File operations
    "ls", "dir", "cat", "head", "tail", "cp", "mv", "rm", "mkdir", "touch", "chmod", "chown",
    "find", "ln", "stat", "pwd", "wc", "du", "readlink", "file", "sha256sum", "tee", "md5sum",
    // Network
    "ifconfig", "ip", "ping", "curl", "wget", "netstat", "ss", "nslookup", "dig", "hostname",
    "route",
    // System
    "mount", "df", "free", "uname", "date", "id", "kill", "printenv", "export", "unset",
    "sleep", "echo", "env", "whoami", "which", "type", "lsof", "true", "false", "clear",
    // Text processing
    "sort", "uniq", "cut", "tr", "sed", "rev", "base64", "xxd", "grep",
];

/// Execute a command in standalone mode (busybox multi-call binary).
/// Returns exit code (0 = success).
pub async fn execute_standalone(cmd: &str, args: &[String]) -> i32 {
    use std::io::{IsTerminal, Read};

    // Read piped stdin if not a terminal
    let piped_input = if !std::io::stdin().is_terminal() {
        let mut input = String::new();
        if std::io::stdin().read_to_string(&mut input).is_ok() && !input.is_empty() {
            Some(input)
        } else {
            None
        }
    } else {
        None
    };

    let args_str: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let container_name = std::env::var("HOSTNAME").unwrap_or_else(|_| "stormd".into());

    let output = match cmd {
        // File operations
        "ls" | "dir" => file::cmd_ls(&args_str).await,
        "cat" => file::cmd_cat(&args_str, piped_input.as_deref()).await,
        "head" => file::cmd_head(&args_str, piped_input.as_deref()).await,
        "tail" => file::cmd_tail(&args_str, piped_input.as_deref()).await,
        "cp" => file::cmd_cp(&args_str).await,
        "mv" => file::cmd_mv(&args_str).await,
        "rm" => file::cmd_rm(&args_str).await,
        "mkdir" => file::cmd_mkdir(&args_str).await,
        "touch" => file::cmd_touch(&args_str).await,
        "chmod" => file::cmd_chmod(&args_str).await,
        "chown" => file::cmd_chown(&args_str).await,
        "find" => file::cmd_find(&args_str).await,
        "ln" => file::cmd_ln(&args_str).await,
        "stat" => file::cmd_stat(&args_str).await,
        "pwd" => file::cmd_pwd(),
        "wc" => file::cmd_wc(&args_str, piped_input.as_deref()).await,
        "du" => file::cmd_du(&args_str).await,
        "readlink" => file::cmd_readlink(&args_str).await,
        "file" => file::cmd_file_type(&args_str).await,
        "sha256sum" | "md5sum" => file::cmd_sha256sum(&args_str, piped_input.as_deref()).await,
        "tee" => file::cmd_tee(&args_str, piped_input.as_deref()).await,

        // Network
        "ifconfig" => net::cmd_ifconfig(&args_str).await,
        "ip" => net::cmd_ip(&args_str).await,
        "ping" => net::cmd_ping(&args_str).await,
        "curl" | "wget" => net::cmd_curl(&args_str).await,
        "netstat" => net::cmd_netstat(&args_str).await,
        "ss" => net::cmd_ss(&args_str).await,
        "nslookup" | "dig" => net::cmd_nslookup(&args_str).await,
        "hostname" => net::cmd_hostname(&container_name, &args_str),
        "route" => net::cmd_route(&args_str).await,

        // System
        "mount" => sys::cmd_mount(),
        "df" => sys::cmd_df(&args_str),
        "free" => sys::cmd_free(&args_str),
        "uname" => sys::cmd_uname(&args_str),
        "date" => sys::cmd_date(),
        "id" => sys::cmd_id(),
        "kill" => sys::cmd_kill(&args_str),
        "printenv" => sys::cmd_printenv(&args_str),
        "export" => sys::cmd_export(&args_str),
        "unset" => sys::cmd_unset(&args_str),
        "sleep" => sys::cmd_sleep(&args_str).await,
        "echo" => sys::cmd_echo(&args_str),
        "env" => sys::cmd_env(),
        "whoami" => sys::cmd_whoami(),
        "which" => sys::cmd_which(&args_str),
        "type" => sys::cmd_type(&args_str),
        "lsof" => sys::cmd_lsof(),
        "true" => ShellOutput::text(""),
        "false" => { print!(""); return 1; }
        "clear" => ShellOutput::text("\x1b[2J\x1b[H"),

        // Text processing
        "sort" => text::cmd_sort(&args_str, piped_input.as_deref()).await,
        "uniq" => text::cmd_uniq(&args_str, piped_input.as_deref()).await,
        "cut" => text::cmd_cut(&args_str, piped_input.as_deref()).await,
        "tr" => text::cmd_tr(&args_str, piped_input.as_deref()).await,
        "sed" => text::cmd_sed(&args_str, piped_input.as_deref()).await,
        "rev" => text::cmd_rev(piped_input.as_deref()),
        "base64" => text::cmd_base64(&args_str, piped_input.as_deref()),
        "xxd" => text::cmd_xxd(piped_input.as_deref()),
        "grep" => text::cmd_grep(&args_str, piped_input.as_deref()).await,

        _ => {
            eprintln!("{}: not available in standalone mode (use stormd shell)", cmd);
            return 127;
        }
    };

    // Convert \r\n to \n for real terminal output
    let text = output.text.replace("\r\n", "\n");
    print!("{}", text);
    0
}

/// Install symlinks for all standalone commands into the given directory.
/// Creates the directory if it doesn't exist. Returns number of links created.
pub fn install_symlinks(dir: &std::path::Path, binary_path: &std::path::Path) -> std::io::Result<usize> {
    std::fs::create_dir_all(dir)?;
    let mut count = 0;
    for cmd in STANDALONE_COMMANDS {
        let link_path = dir.join(cmd);
        // Skip if already exists
        if link_path.exists() || link_path.symlink_metadata().is_ok() {
            continue;
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(binary_path, &link_path)?;
            count += 1;
        }
        #[cfg(not(unix))]
        {
            let _ = link_path;
            let _ = binary_path;
        }
    }
    Ok(count)
}

pub(crate) fn format_duration(secs: i64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    if days > 0 {
        format!("{}d {}h {}m", days, hours, mins)
    } else if hours > 0 {
        format!("{}h {}m {}s", hours, mins, s)
    } else if mins > 0 {
        format!("{}m {}s", mins, s)
    } else {
        format!("{}s", s)
    }
}

/// Tab completion for commands, process names, and file paths.
pub async fn complete(partial: &str, state: &Arc<AppState>) -> Vec<String> {
    let parts: Vec<&str> = partial.split_whitespace().collect();

    if parts.len() <= 1 {
        // Complete command name
        let prefix = parts.first().copied().unwrap_or("");
        return ALL_COMMANDS
            .iter()
            .filter(|c| c.starts_with(prefix))
            .map(|c| c.to_string())
            .collect();
    }

    let cmd = parts[0];

    // systemctl subcommand completion
    if cmd == "systemctl" && parts.len() == 2 {
        let prefix = parts[1];
        return [
            "status",
            "start",
            "stop",
            "restart",
            "list-units",
            "is-active",
            "is-failed",
            "enable",
            "disable",
        ]
        .iter()
        .filter(|c| c.starts_with(prefix))
        .map(|c| c.to_string())
        .collect();
    }

    // Process name completion for supervisor commands
    if matches!(
        cmd,
        "start" | "stop" | "restart" | "attach" | "logs" | "liveness"
    ) || (cmd == "systemctl" && parts.len() >= 3)
    {
        let prefix = parts.last().copied().unwrap_or("");
        let names = state.supervisor.process_names().await;
        return names
            .into_iter()
            .filter(|n| n.starts_with(prefix))
            .collect();
    }

    // File path completion for file-related commands
    let file_cmds = [
        "cat", "head", "tail", "ls", "cp", "mv", "rm", "mkdir", "touch", "chmod", "chown",
        "find", "ln", "stat", "du", "readlink", "file", "sha256sum", "wc", "tee",
    ];
    if file_cmds.contains(&cmd) {
        let prefix = parts.last().copied().unwrap_or("");
        return complete_path(prefix).await;
    }

    Vec::new()
}

async fn complete_path(prefix: &str) -> Vec<String> {
    use std::path::Path;

    let (dir, file_prefix) = if prefix.contains('/') {
        let path = Path::new(prefix);
        let parent = path.parent().unwrap_or(Path::new("/"));
        let file_part = path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();
        (parent.to_path_buf(), file_part)
    } else if prefix.is_empty() {
        (
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/")),
            String::new(),
        )
    } else {
        (
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/")),
            prefix.to_string(),
        )
    };

    let mut results = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&file_prefix) {
                let full = if prefix.contains('/') {
                    format!("{}/{}", dir.display(), name)
                } else {
                    name
                };
                if let Ok(ft) = entry.file_type().await {
                    if ft.is_dir() {
                        results.push(format!("{}/", full));
                    } else {
                        results.push(full);
                    }
                } else {
                    results.push(full);
                }
            }
        }
    }
    results.sort();
    results
}
