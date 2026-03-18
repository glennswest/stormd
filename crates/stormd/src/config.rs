use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use stormlog::types::StormLogConfig;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub process: Vec<ProcessConfig>,
    #[serde(default)]
    pub cron: Vec<CronJobConfig>,
    #[serde(default)]
    pub events: EventsConfig,
    #[serde(default)]
    pub backup: BackupConfig,
    #[serde(default)]
    pub log: LogConfig,
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub debug: DebugConfig,
    #[serde(default)]
    pub stormlog: StormLogConfig,
    #[serde(default)]
    pub ssh: SshConfig,
    #[serde(default)]
    pub updater: UpdaterConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(default = "default_log_dir")]
    pub log_dir: PathBuf,
    #[serde(default = "default_pid_file")]
    pub pid_file: PathBuf,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            name: default_name(),
            log_dir: default_log_dir(),
            pid_file: default_pid_file(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProcessConfig {
    pub name: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub working_dir: Option<PathBuf>,
    #[serde(default = "default_on_failure")]
    pub on_failure: FailureAction,
    #[serde(default = "default_on_exit")]
    pub on_exit: ExitAction,
    #[serde(default = "default_restart_delay_secs")]
    pub restart_delay_secs: u64,
    #[serde(default = "default_max_restarts")]
    pub max_restarts: u32,
    #[serde(default = "default_restart_window_secs")]
    pub restart_window_secs: u64,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default = "default_startup_delay_secs")]
    pub startup_delay_secs: u64,
    #[serde(default)]
    pub ready_probe: Option<ReadyProbe>,
    #[serde(default)]
    pub liveness: Option<LivenessProbe>,
    #[serde(default = "default_true")]
    pub capture_stdout: bool,
    #[serde(default = "default_true")]
    pub capture_stderr: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FailureAction {
    Restart,
    Fail,
    Ignore,
}

/// What to do when a process exits cleanly (exit code 0).
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ExitAction {
    Restart,
    Stop,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ReadyProbe {
    Http { url: String, interval_secs: u64 },
    Tcp { port: u16, interval_secs: u64 },
    Exec { command: String, interval_secs: u64 },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ProbeType {
    Http { url: String },
    Tcp { port: u16 },
}

#[derive(Debug, Clone, Deserialize)]
pub struct LivenessProbe {
    #[serde(flatten)]
    pub probe: ProbeType,
    #[serde(default = "default_liveness_interval")]
    pub interval_secs: u64,
    #[serde(default = "default_liveness_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: u32,
    #[serde(default = "default_initial_delay")]
    pub initial_delay_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CronJobConfig {
    pub name: String,
    pub schedule: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default = "default_cron_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_true")]
    pub capture_output: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub transport: EventTransport,
    pub nats_url: Option<String>,
    #[serde(default = "default_nats_subject")]
    pub nats_subject: String,
    pub webhook_url: Option<String>,
    #[serde(default)]
    pub webhook_headers: HashMap<String, String>,
}

impl Default for EventsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            transport: EventTransport::None,
            nats_url: None,
            nats_subject: default_nats_subject(),
            webhook_url: None,
            webhook_headers: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum EventTransport {
    #[default]
    None,
    Nats,
    Webhook,
    Both,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BackupConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub on_failure: bool,
    pub destination_url: Option<String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default = "default_true")]
    pub compress: bool,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            on_failure: true,
            destination_url: None,
            headers: HashMap::new(),
            compress: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct LogConfig {
    #[serde(default = "default_max_size_bytes")]
    pub max_size_bytes: u64,
    #[serde(default = "default_max_files")]
    pub max_files: u32,
    #[serde(default = "default_true")]
    pub timestamps: bool,
    #[serde(default)]
    pub json_format: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            max_size_bytes: default_max_size_bytes(),
            max_files: default_max_files(),
            timestamps: true,
            json_format: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiConfig {
    #[serde(default = "default_api_bind")]
    pub bind: String,
    #[serde(default)]
    pub auth_token: Option<String>,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            bind: default_api_bind(),
            auth_token: None,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DebugConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub allow_signal: bool,
    #[serde(default)]
    pub allow_stdin: bool,
    #[serde(default)]
    pub dynamic_log_level: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SshConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_ssh_bind")]
    pub bind: String,
    #[serde(default = "default_ssh_host_key")]
    pub host_key: PathBuf,
    #[serde(default = "default_ssh_password")]
    pub password: String,
    pub authorized_keys: Option<PathBuf>,
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: default_ssh_bind(),
            host_key: default_ssh_host_key(),
            password: default_ssh_password(),
            authorized_keys: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdaterConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_updater_registry")]
    pub registry: String,
    #[serde(default = "default_updater_poll_interval")]
    pub poll_interval_secs: u64,
    #[serde(default = "default_updater_data_dir")]
    pub data_dir: PathBuf,
    #[serde(default = "default_updater_rootfs_dir")]
    pub rootfs_dir: PathBuf,
}

impl Default for UpdaterConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            registry: default_updater_registry(),
            poll_interval_secs: default_updater_poll_interval(),
            data_dir: default_updater_data_dir(),
            rootfs_dir: default_updater_rootfs_dir(),
        }
    }
}

fn default_updater_registry() -> String { "registry.gt.lo".to_string() }
fn default_updater_poll_interval() -> u64 { 60 }
fn default_updater_data_dir() -> PathBuf { PathBuf::from("/data/images") }
fn default_updater_rootfs_dir() -> PathBuf { PathBuf::from("/data/rootfs") }

fn default_name() -> String { "stormd".to_string() }
fn default_log_dir() -> PathBuf { PathBuf::from("/var/log/stormd") }
fn default_pid_file() -> PathBuf { PathBuf::from("/run/stormd.pid") }
fn default_on_failure() -> FailureAction { FailureAction::Restart }
fn default_on_exit() -> ExitAction { ExitAction::Restart }
fn default_restart_delay_secs() -> u64 { 1 }
fn default_max_restarts() -> u32 { 10 }
fn default_restart_window_secs() -> u64 { 3600 }
fn default_startup_delay_secs() -> u64 { 0 }
fn default_cron_timeout_secs() -> u64 { 300 }
fn default_nats_subject() -> String { "stormd.events".to_string() }
fn default_max_size_bytes() -> u64 { 100 * 1024 * 1024 }
fn default_max_files() -> u32 { 10 }
fn default_api_bind() -> String { "0.0.0.0:9080".to_string() }
fn default_ssh_bind() -> String { "0.0.0.0:22".to_string() }
fn default_ssh_host_key() -> PathBuf { PathBuf::from("/etc/stormd/host_key") }
fn default_ssh_password() -> String { "stormd".to_string() }
fn default_liveness_interval() -> u64 { 10 }
fn default_liveness_timeout() -> u64 { 5 }
fn default_failure_threshold() -> u32 { 1 }
fn default_initial_delay() -> u64 { 5 }
fn default_true() -> bool { true }

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.process.is_empty() && self.cron.is_empty() {
            anyhow::bail!("at least one process or cron job must be configured");
        }
        let mut names = std::collections::HashSet::new();
        for p in &self.process {
            if !names.insert(&p.name) {
                anyhow::bail!("duplicate process name: {}", p.name);
            }
            if p.command.is_empty() && p.image.is_none() {
                anyhow::bail!("process '{}' must have either command or image set", p.name);
            }
        }
        for dep in self.process.iter().flat_map(|p| &p.depends_on) {
            if !names.contains(dep) {
                anyhow::bail!("unknown dependency: {}", dep);
            }
        }
        if self.events.enabled {
            match self.events.transport {
                EventTransport::Nats => {
                    if self.events.nats_url.is_none() {
                        anyhow::bail!("NATS transport enabled but nats_url not set");
                    }
                }
                EventTransport::Webhook => {
                    if self.events.webhook_url.is_none() {
                        anyhow::bail!("webhook transport enabled but webhook_url not set");
                    }
                }
                EventTransport::Both => {
                    if self.events.nats_url.is_none() {
                        anyhow::bail!("NATS transport enabled but nats_url not set");
                    }
                    if self.events.webhook_url.is_none() {
                        anyhow::bail!("webhook transport enabled but webhook_url not set");
                    }
                }
                EventTransport::None => {}
            }
        }
        if self.backup.enabled && self.backup.destination_url.is_none() {
            anyhow::bail!("backup enabled but destination_url not set");
        }
        Ok(())
    }
}
