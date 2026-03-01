use crate::types::{LogEntry, LogStream, Severity, SyslogConfig};
use tokio::io::AsyncBufReadExt;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Syslog receiver — listens on UDP, TCP, and Unix domain socket.
pub struct SyslogReceiver {
    config: SyslogConfig,
    entry_tx: mpsc::Sender<LogEntry>,
}

impl SyslogReceiver {
    pub fn new(config: SyslogConfig, entry_tx: mpsc::Sender<LogEntry>) -> Self {
        Self { config, entry_tx }
    }

    /// Start all configured syslog listeners.
    pub async fn start(&self) {
        if !self.config.enabled {
            return;
        }

        // UDP listener
        let udp_bind = self.config.udp_bind.clone();
        let tx = self.entry_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = run_udp_listener(&udp_bind, tx).await {
                error!(error = %e, "syslog UDP listener failed");
            }
        });

        // TCP listener
        let tcp_bind = self.config.tcp_bind.clone();
        let tx = self.entry_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = run_tcp_listener(&tcp_bind, tx).await {
                error!(error = %e, "syslog TCP listener failed");
            }
        });

        // Unix domain socket (Linux only)
        #[cfg(target_os = "linux")]
        {
            let socket_path = self.config.unix_socket.clone();
            let tx = self.entry_tx.clone();
            tokio::spawn(async move {
                if let Err(e) = run_unix_listener(&socket_path, tx).await {
                    error!(error = %e, "syslog Unix socket listener failed");
                }
            });
        }

        info!("syslog listeners started");
    }
}

async fn run_udp_listener(bind: &str, tx: mpsc::Sender<LogEntry>) -> anyhow::Result<()> {
    let socket = UdpSocket::bind(bind).await?;
    info!(addr = %bind, "syslog UDP listening");

    let mut buf = [0u8; 8192];
    loop {
        let (len, _addr) = match socket.recv_from(&mut buf).await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "syslog UDP recv error");
                continue;
            }
        };

        let msg = String::from_utf8_lossy(&buf[..len]);
        if let Some(entry) = parse_syslog_message(&msg) {
            let _ = tx.send(entry).await;
        }
    }
}

async fn run_tcp_listener(bind: &str, tx: mpsc::Sender<LogEntry>) -> anyhow::Result<()> {
    let listener = TcpListener::bind(bind).await?;
    info!(addr = %bind, "syslog TCP listening");

    loop {
        let (stream, _addr) = listener.accept().await?;
        let tx = tx.clone();
        tokio::spawn(async move {
            let reader = tokio::io::BufReader::new(stream);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(entry) = parse_syslog_message(&line) {
                    let _ = tx.send(entry).await;
                }
            }
        });
    }
}

#[cfg(target_os = "linux")]
async fn run_unix_listener(path: &str, tx: mpsc::Sender<LogEntry>) -> anyhow::Result<()> {
    // Remove existing socket file
    let _ = tokio::fs::remove_file(path).await;

    let listener = tokio::net::UnixListener::bind(path)?;
    info!(path = %path, "syslog Unix socket listening");

    loop {
        let (stream, _) = listener.accept().await?;
        let tx = tx.clone();
        tokio::spawn(async move {
            let reader = tokio::io::BufReader::new(stream);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(entry) = parse_syslog_message(&line) {
                    let _ = tx.send(entry).await;
                }
            }
        });
    }
}

/// Parse a syslog message (RFC 3164 / RFC 5424 style).
///
/// Format: `<PRI>TIMESTAMP HOSTNAME APP-NAME[PID]: MESSAGE`
/// or simplified: `<PRI>MESSAGE`
fn parse_syslog_message(raw: &str) -> Option<LogEntry> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    // Try to parse priority: <PRI>
    if raw.starts_with('<') {
        if let Some(end) = raw.find('>') {
            let pri_str = &raw[1..end];
            let priority: u8 = pri_str.parse().unwrap_or(13); // default to user.notice
            let severity = Severity::from_syslog_priority(priority);
            let facility_name = facility_name((priority >> 3) & 0x1f);

            let rest = &raw[end + 1..];

            // Try RFC 3164: TIMESTAMP HOSTNAME APP-NAME[PID]: MESSAGE
            // Simplified: just grab the process name and message
            let (process, message) = parse_rfc3164_body(rest);

            let process = if process.is_empty() {
                facility_name.to_string()
            } else {
                process
            };

            return Some(
                LogEntry::new(process, LogStream::Syslog, message).with_severity(severity),
            );
        }
    }

    // Fallback: treat entire line as a message from "syslog"
    Some(LogEntry::new("syslog", LogStream::Syslog, raw).with_severity(Severity::Notice))
}

fn parse_rfc3164_body(body: &str) -> (String, String) {
    let body = body.trim();

    // Skip timestamp (e.g., "Feb 28 12:34:56")
    // Try to find the pattern: skip 3 space-delimited fields (month day time), then hostname, then tag
    let parts: Vec<&str> = body.splitn(5, ' ').collect();
    if parts.len() >= 5 {
        // parts[0..3] = timestamp, parts[3] = hostname, parts[4] = tag: msg
        let rest = parts[4];
        if let Some(colon_pos) = rest.find(':') {
            let tag = &rest[..colon_pos];
            let msg = rest[colon_pos + 1..].trim();
            // Strip PID from tag: "app[1234]" -> "app"
            let process = tag.split('[').next().unwrap_or(tag).to_string();
            return (process, msg.to_string());
        }
        return (String::new(), rest.to_string());
    }

    // Try simpler format: TAG: MESSAGE or TAG[PID]: MESSAGE
    if let Some(colon_pos) = body.find(':') {
        let tag = &body[..colon_pos];
        let msg = body[colon_pos + 1..].trim();
        let process = tag.split('[').next().unwrap_or(tag).trim().to_string();
        return (process, msg.to_string());
    }

    (String::new(), body.to_string())
}

fn facility_name(facility: u8) -> &'static str {
    match facility {
        0 => "kern",
        1 => "user",
        2 => "mail",
        3 => "daemon",
        4 => "auth",
        5 => "syslog",
        6 => "lpr",
        7 => "news",
        8 => "uucp",
        9 => "cron",
        10 => "authpriv",
        11 => "ftp",
        16 => "local0",
        17 => "local1",
        18 => "local2",
        19 => "local3",
        20 => "local4",
        21 => "local5",
        22 => "local6",
        23 => "local7",
        _ => "unknown",
    }
}
