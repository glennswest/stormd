use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use stormlog::types::StormLogConfig;
use tracing::info;

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
    #[serde(default)]
    pub goldens: GoldensConfig,
}

/// Where stormd attaches the goldens processes name (`[[process.golden]]`,
/// stormd#36): the node's stormblock engine.
#[derive(Debug, Clone, Deserialize)]
pub struct GoldensConfig {
    /// The engine's API. `${NODE_IP}`/`${NODE_NAME}` are expanded. A ublk
    /// attach is local, so this must be the engine on this node.
    #[serde(default = "default_goldens_engine_url")]
    pub engine_url: String,
    /// The engine's bearer token, re-read on every call. Absent: no header.
    #[serde(default = "default_goldens_token_file")]
    pub token_file: PathBuf,
    /// Where a golden is presented when it names no `path`: `<dir>/<name>`.
    #[serde(default = "default_goldens_dir")]
    pub dir: PathBuf,
}

impl Default for GoldensConfig {
    fn default() -> Self {
        Self {
            engine_url: default_goldens_engine_url(),
            token_file: default_goldens_token_file(),
            dir: default_goldens_dir(),
        }
    }
}

fn default_goldens_engine_url() -> String { "http://${NODE_IP}:9090".into() }
fn default_goldens_token_file() -> PathBuf { PathBuf::from("/run/stormblock/engine/api_token") }
fn default_goldens_dir() -> PathBuf { PathBuf::from("/goldens") }

/// A golden a process is given, read-only (stormd#36).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct GoldenMount {
    /// How the process refers to it; the default path is `<dir>/<name>`.
    pub name: String,
    /// The golden's name in stormblock, e.g. `golden-nic-drivers-56ea4782ef2a`.
    #[serde(default)]
    pub golden: Option<String>,
    /// Or its volume id.
    #[serde(default)]
    pub volume_id: Option<String>,
    pub content: GoldenContent,
    /// Where it appears: a read-only mount (filesystem) or a device node (image).
    #[serde(default)]
    pub path: Option<String>,
    /// Filesystem goldens: the type to mount.
    #[serde(default = "default_golden_fstype")]
    pub fstype: String,
    /// Image goldens: `uid:gid` of the device node, so an unprivileged
    /// service can read it.
    #[serde(default)]
    pub owner: Option<String>,
    /// Image goldens: the device node's mode.
    #[serde(default = "default_golden_mode")]
    pub mode: u32,
    /// Image goldens: the image's own length (the volume is larger). Reported
    /// with the golden; the service limits what it serves to it.
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GoldenContent {
    Filesystem,
    Image,
}

fn default_golden_fstype() -> String { "ext4".into() }
fn default_golden_mode() -> u32 { 0o444 }

impl GoldenMount {
    /// `owner` as (uid, gid); `None` when unset or malformed.
    pub fn owner_ids(&self) -> Option<(u32, u32)> {
        let (u, g) = self.owner.as_deref()?.split_once(':')?;
        Some((u.parse().ok()?, g.parse().ok()?))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(default = "default_log_dir")]
    pub log_dir: PathBuf,
    #[serde(default = "default_pid_file")]
    pub pid_file: PathBuf,
    #[serde(default)]
    pub cloud_id: Option<String>,
    /// Default web UI theme ("storm", "midnight", "catppuccin", "rose",
    /// "nord", "solar", "phosphor", "light"). A viewer's own pick, stored in
    /// their browser, wins over this.
    #[serde(default)]
    pub theme: Option<String>,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            name: default_name(),
            log_dir: default_log_dir(),
            pid_file: default_pid_file(),
            cloud_id: None,
            theme: None,
        }
    }
}

/// Resolve the cloud_id for this instance.
///
/// Priority order:
/// 1. Explicit value in config (`[general] cloud_id = "..."`)
/// 2. Environment variable `STORM_CLOUD_ID`
/// 3. Persisted file at `{log_dir}/.cloudid`
/// 4. Generate a new UUID v4 and persist to `{log_dir}/.cloudid`
pub fn resolve_cloud_id(config: &GeneralConfig) -> String {
    // 1. Config file
    if let Some(ref id) = config.cloud_id {
        if !id.is_empty() {
            info!(cloud_id = %id, source = "config", "cloud_id resolved");
            return id.clone();
        }
    }

    // 2. Environment variable
    if let Ok(id) = std::env::var("STORM_CLOUD_ID") {
        if !id.is_empty() {
            info!(cloud_id = %id, source = "env", "cloud_id resolved");
            return id;
        }
    }

    // 3. Persisted file
    let persist_path = config.log_dir.join(".cloudid");
    if let Ok(id) = std::fs::read_to_string(&persist_path) {
        let id = id.trim().to_string();
        if !id.is_empty() {
            info!(cloud_id = %id, source = "file", path = %persist_path.display(), "cloud_id resolved");
            return id;
        }
    }

    // 4. Generate new UUID and persist
    let id = uuid::Uuid::new_v4().to_string();
    if let Err(e) = std::fs::create_dir_all(&config.log_dir) {
        tracing::warn!(error = %e, "could not create log_dir for cloud_id persistence");
    }
    if let Err(e) = std::fs::write(&persist_path, &id) {
        tracing::warn!(error = %e, path = %persist_path.display(), "could not persist cloud_id");
    }
    info!(cloud_id = %id, source = "generated", "cloud_id resolved");
    id
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
    /// Applied only when the key is not in stormd's own environment, so a
    /// node can override it (stormpump's env.d); `env` still wins over both.
    #[serde(default)]
    pub env_default: HashMap<String, String>,
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
    /// Exit codes the process uses to say a restart will not help — a config
    /// it cannot run on, usually. sysexits gives 78 (EX_CONFIG) and 64
    /// (EX_USAGE); stormconsole exits 78. An exit with one of these is not
    /// restarted and does not count toward `max_restarts`; what happens to
    /// the container is `on_no_restart`. Empty by default, so nothing changes
    /// for a config that does not say.
    #[serde(default)]
    pub no_restart_exit_codes: Vec<i32>,
    #[serde(default)]
    pub on_no_restart: NoRestartAction,
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Files that must all exist before the process is first started — a
    /// cert pair another container mints, say (stormd#38). Absolute paths;
    /// `${NODE_IP}`/`${NODE_NAME}` are expanded.
    #[serde(default)]
    pub wait_for_files: Vec<String>,
    /// Goldens presented to this process, read-only, before it first starts
    /// (stormd#36).
    #[serde(default)]
    pub golden: Vec<GoldenMount>,
    /// How long a stop (API, shell, restart, updater pivot, shutdown) waits
    /// after SIGTERM before SIGKILL. 0 is SIGKILL at once.
    #[serde(default = "default_stop_timeout_secs")]
    pub stop_timeout_secs: u64,
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
    #[serde(default)]
    pub ui: Option<ProcessUiConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProcessUiConfig {
    pub label: String,
    pub proxy: String,
    /// Optional virtual host. Requests whose `Host:` matches on the API port are
    /// routed to this plugin's UI, enabling name-based routing (e.g. clients.mob.lo).
    #[serde(default)]
    pub host: Option<String>,
    /// Optional URL returning the plugin's own component summary (JSON with
    /// any of `health`, `detail`, `metrics`), merged into its dashboard card.
    /// Best-effort with a short timeout — a slow or absent endpoint costs the
    /// card nothing but the extra detail.
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FailureAction {
    Restart,
    Fail,
    Ignore,
}

/// What to do with the container when a process exits with one of its
/// `no_restart_exit_codes`. `hold` leaves the container running with the
/// process marked Failed — visible on every dashboard, and nothing outside
/// is invited to restart what a restart cannot fix. `fail` fails the
/// container for the supervisor above to deal with.
#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum NoRestartAction {
    #[default]
    Hold,
    Fail,
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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ProbeType {
    Http { url: String },
    Tcp { port: u16 },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
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
    pub webhook_url: Option<String>,
    #[serde(default)]
    pub webhook_headers: HashMap<String, String>,
}

impl Default for EventsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            transport: EventTransport::None,
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
    Webhook,
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
    /// Machine credential: `Authorization: Bearer <auth_token>` on any
    /// request, and it also works as the "admin" login password. Setting this,
    /// `password`, or any `[[api.users]]` turns authentication on.
    #[serde(default)]
    pub auth_token: Option<String>,
    /// Legacy interactive credential — equivalent to a user named "admin"
    /// with this password. Prefer [[api.users]].
    #[serde(default)]
    pub password: Option<String>,
    /// Named users for the UI login. Any user (or `password`/`auth_token`
    /// above) being configured turns authentication on.
    #[serde(default)]
    pub users: Vec<ApiUser>,
    /// The machine bearer token, read from a file (whitespace trimmed) and
    /// re-read when the file changes. Works like `auth_token`; both may be set.
    #[serde(default)]
    pub token_file: Option<PathBuf>,
    /// Serve the API over TLS with this PEM certificate chain and key. Both or
    /// neither; re-read when either file changes (stormcert rotation).
    #[serde(default)]
    pub tls_cert_file: Option<PathBuf>,
    #[serde(default)]
    pub tls_key_file: Option<PathBuf>,
    /// With TLS: a client certificate that verifies against this PEM CA
    /// bundle authenticates the request. Turns authentication on. Needs TLS.
    #[serde(default)]
    pub client_ca_file: Option<PathBuf>,
    /// Reusable, config-driven host-based routing: `Host:` header -> redirect
    /// target path (e.g. "manager.mob.lo" = "/ui/", "api.x" = "/api/v1/health").
    #[serde(default)]
    pub hosts: std::collections::HashMap<String, String>,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            bind: default_api_bind(),
            auth_token: None,
            password: None,
            users: Vec::new(),
            token_file: None,
            tls_cert_file: None,
            tls_key_file: None,
            client_ca_file: None,
            hosts: std::collections::HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiUser {
    pub name: String,
    pub password: String,
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
    /// CloudID metadata endpoint URL for SSH public key auth.
    /// Defaults to the EC2-compatible metadata IP (169.254.169.254).
    #[serde(default = "default_cloudid_url")]
    pub cloudid_url: String,
    /// Owner tag — identifies this container to CloudID for key resolution.
    /// When set, CloudID uses the namespace owner annotation to serve the
    /// correct SSH keys. Required for CloudID public key auth to activate.
    #[serde(default)]
    pub owner: Option<String>,
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: default_ssh_bind(),
            host_key: default_ssh_host_key(),
            password: default_ssh_password(),
            authorized_keys: None,
            cloudid_url: default_cloudid_url(),
            owner: None,
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
fn default_stop_timeout_secs() -> u64 { 10 }
fn default_max_restarts() -> u32 { 10 }
fn default_restart_window_secs() -> u64 { 3600 }
fn default_startup_delay_secs() -> u64 { 0 }
fn default_cron_timeout_secs() -> u64 { 300 }
fn default_max_size_bytes() -> u64 { 100 * 1024 * 1024 }
fn default_max_files() -> u32 { 10 }
fn default_api_bind() -> String { "0.0.0.0:9080".to_string() }
fn default_ssh_bind() -> String { "0.0.0.0:22".to_string() }
fn default_ssh_host_key() -> PathBuf { PathBuf::from("/etc/stormd/host_key") }
fn default_ssh_password() -> String { "stormd".to_string() }
fn default_cloudid_url() -> String { "http://169.254.169.254".to_string() }
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
        for p in &self.process {
            if let Some(f) = p.wait_for_files.iter().find(|f| !f.starts_with('/')) {
                anyhow::bail!("process '{}': wait_for_files entry '{}' is not an absolute path", p.name, f);
            }
            let mut seen = std::collections::HashSet::new();
            for g in &p.golden {
                let at = format!("process '{}', golden '{}'", p.name, g.name);
                if g.name.is_empty() || g.name.contains('/') || !seen.insert(&g.name) {
                    anyhow::bail!("{at}: name must be non-empty, unique in the process, and have no '/'");
                }
                if g.golden.is_some() == g.volume_id.is_some() {
                    anyhow::bail!("{at}: set exactly one of golden and volume_id");
                }
                if g.path.as_deref().is_some_and(|p| !p.starts_with('/')) {
                    anyhow::bail!("{at}: path must be absolute");
                }
                if g.owner.is_some() && g.owner_ids().is_none() {
                    anyhow::bail!("{at}: owner must be uid:gid (numbers)");
                }
            }
        }
        if self.events.enabled {
            match self.events.transport {
                EventTransport::Webhook => {
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
        if self.api.tls_cert_file.is_some() != self.api.tls_key_file.is_some() {
            anyhow::bail!("[api] tls_cert_file and tls_key_file must be set together");
        }
        if self.api.client_ca_file.is_some() && self.api.tls_cert_file.is_none() {
            anyhow::bail!("[api] client_ca_file needs TLS (tls_cert_file + tls_key_file)");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped example is documentation, and documentation that stormd
    /// refuses to start on is worse than none: it said `transport = "nats"`
    /// for months after NATS was removed, and every copy of it failed to load.
    #[test]
    fn the_example_config_parses_and_validates() {
        let c: Config = toml::from_str(include_str!("../../../config/example.toml"))
            .expect("config/example.toml must parse");
        c.validate().expect("config/example.toml must validate");
        assert!(c.process.iter().any(|p| p.ready_probe.is_some()));
        assert!(c.process.iter().any(|p| p.ui.is_some()));
        assert!(c.process.iter().any(|p| !p.wait_for_files.is_empty()));
    }

    #[test]
    fn wait_for_files_must_be_absolute() {
        let parse = |files: &str| -> anyhow::Result<()> {
            let c: Config = toml::from_str(&format!(
                "[[process]]\nname = \"p\"\ncommand = \"/bin/true\"\nwait_for_files = {files}\n"
            ))?;
            c.validate()
        };
        assert!(parse(r#"["/etc/stormcert/fastetcd.crt", "/etc/${NODE_NAME}.key"]"#).is_ok());
        let e = parse(r#"["/etc/ok", "stormcert/fastetcd.crt"]"#).unwrap_err().to_string();
        assert!(e.contains("not an absolute path"), "{e}");
    }
}
