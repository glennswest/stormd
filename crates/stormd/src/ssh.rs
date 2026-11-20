use crate::api::AppState;
use crate::cloudid::SshKeyStore;
use crate::config::SshConfig;
use crate::shell;
use async_trait::async_trait;
use chrono::Utc;
use russh::server::{Auth, Handler, Msg, Server, Session};
use russh::{Channel, ChannelId, CryptoVec, MethodSet};
use russh_keys::key::{KeyPair, PublicKey};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{error, info, warn};

/// Start the SSH server.
pub async fn start_ssh_server(
    config: SshConfig,
    app_state: Arc<AppState>,
    container_name: String,
    cloudid_keys: Option<Arc<RwLock<SshKeyStore>>>,
) {
    if !config.enabled {
        return;
    }

    let key_pair = load_or_generate_host_key(&config.host_key).await;

    let russh_config = russh::server::Config {
        auth_rejection_time: std::time::Duration::from_secs(1),
        auth_rejection_time_initial: Some(std::time::Duration::from_secs(0)),
        keys: vec![key_pair],
        ..Default::default()
    };

    let mut server = SshServer {
        config: config.clone(),
        app_state,
        container_name,
        cloudid_keys,
    };

    info!(addr = %config.bind, "SSH server listening");

    if let Err(e) = server
        .run_on_address(Arc::new(russh_config), config.bind.as_str())
        .await
    {
        error!(error = %e, "SSH server error");
    }
}

/// The host key at `path`, or a new ed25519 one saved there (stormd#86).
///
/// **The saved private key was world-readable.** It was written with
/// `fs::write`, so with the usual 022 umask it was 0644, and anything that
/// could read the container filesystem could copy it and impersonate the
/// server. It is now written 0600 (a temp file created exclusively, then
/// renamed), an existing one with group or other bits is tightened to 0600
/// when loaded, and a symlink at `path` is neither followed nor replaced.
async fn load_or_generate_host_key(path: &std::path::Path) -> KeyPair {
    match read_host_key(path) {
        Ok(Some(key)) => {
            info!(path = %path.display(), "loaded SSH host key");
            return key;
        }
        Ok(None) => {}
        Err(e) => warn!(path = %path.display(), error = %e, "SSH host key not loaded — generating a new one"),
    }

    // Generate new Ed25519 key
    let key = KeyPair::generate_ed25519();

    // Try to save it
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }

    // Save private key in PKCS8 PEM format
    let mut buf = Vec::new();
    if let Err(e) = russh_keys::encode_pkcs8_pem(&key, &mut buf) {
        warn!(error = %e, path = %path.display(), "failed to encode host key");
    } else if let Err(e) = save_host_key(path, &buf) {
        warn!(error = %e, path = %path.display(), "failed to save host key — this one lasts until stormd restarts");
    } else {
        info!(path = %path.display(), "generated new SSH host key");
    }

    key
}

/// Read the private key at `path` without following a symlink: `None` when
/// there is none. A key readable by group or other is made 0600 first.
fn read_host_key(path: &std::path::Path) -> Result<Option<KeyPair>, String> {
    use std::io::Read;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut f = match std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => return Err("it is a symlink — not followed".into()),
        Err(e) => return Err(e.to_string()),
    };
    let meta = f.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        warn!(path = %path.display(), mode = format!("{mode:o}"), "SSH host key readable beyond its owner — made 0600");
        f.set_permissions(std::fs::Permissions::from_mode(0o600)).map_err(|e| format!("cannot make it 0600: {e}"))?;
    }
    let mut text = String::new();
    f.read_to_string(&mut text).map_err(|e| e.to_string())?;
    russh_keys::decode_secret_key(&text, None).map(Some).map_err(|e| e.to_string())
}

/// Write a private key to `path`, 0600 from the moment it exists: a temp
/// file created exclusively (never through a symlink), then renamed over
/// `path`. A symlink at `path` is left alone.
fn save_host_key(path: &std::path::Path, pem: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(std::io::Error::other("it is a symlink — not replaced"));
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&tmp)?;
    f.write_all(pem)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

struct SshServer {
    config: SshConfig,
    app_state: Arc<AppState>,
    container_name: String,
    cloudid_keys: Option<Arc<RwLock<SshKeyStore>>>,
}

impl russh::server::Server for SshServer {
    type Handler = SshSession;

    fn new_client(&mut self, _peer_addr: Option<std::net::SocketAddr>) -> Self::Handler {
        SshSession {
            config: self.config.clone(),
            app_state: self.app_state.clone(),
            container_name: self.container_name.clone(),
            channels: Arc::new(Mutex::new(HashMap::new())),
            raw_channels: Arc::new(Mutex::new(HashMap::new())),
            started_at: Utc::now(),
            cloudid_keys: self.cloudid_keys.clone(),
        }
    }
}

struct ChannelState {
    line_buffer: String,
    history: Vec<String>,
}

struct SshSession {
    config: SshConfig,
    app_state: Arc<AppState>,
    container_name: String,
    channels: Arc<Mutex<HashMap<ChannelId, ChannelState>>>,
    raw_channels: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
    started_at: chrono::DateTime<Utc>,
    cloudid_keys: Option<Arc<RwLock<SshKeyStore>>>,
}

impl SshSession {
    fn send_prompt(&self, session: &mut Session, channel: ChannelId) {
        let prompt = format!(
            "\x1b[1;32mroot@{}\x1b[0m:\x1b[1;34m~\x1b[0m# ",
            self.container_name
        );
        session.data(channel, CryptoVec::from(prompt.into_bytes()));
    }
}

fn cv(data: &[u8]) -> CryptoVec {
    CryptoVec::from(data.to_vec())
}

impl SshSession {
    /// Whether public-key auth is on at all: CloudID keys or an
    /// `authorized_keys` file.
    fn has_keys(&self) -> bool {
        self.cloudid_keys.is_some() || self.config.authorized_keys.is_some()
    }

    /// Whether `[ssh] authorized_keys` lists this key. Read on every call,
    /// so an edited file takes effect at the next login (stormd#7).
    fn in_authorized_keys(&self, key: &PublicKey) -> bool {
        let Some(path) = &self.config.authorized_keys else { return false };
        match std::fs::read_to_string(path) {
            Ok(text) => authorized_keys_contains(&text, key),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "authorized_keys unreadable");
                false
            }
        }
    }
}

/// The `authorized_keys` options stormd accepts (stormd#86): each holds by
/// construction, since stormd's SSH server has no port, agent or X11
/// forwarding and runs no rc file; the positive forms grant nothing it does
/// not already give. Any other option — `from=`, `command=`, `restrict`,
/// `no-pty`, `environment=`, `permitopen=`, `expiry-time=` … — is a
/// restriction stormd would not enforce, so an entry carrying one is refused.
const ENFORCED_OPTIONS: &[&str] = &[
    "no-port-forwarding",
    "no-agent-forwarding",
    "no-x11-forwarding",
    "no-user-rc",
    "port-forwarding",
    "agent-forwarding",
    "x11-forwarding",
    "user-rc",
    "pty",
];

/// One `authorized_keys` line: its options (names, lower-case) and its key.
/// `None` for a line that holds no key.
fn parse_authorized_key(line: &str) -> Option<(Vec<String>, PublicKey)> {
    fn key_type(t: &str) -> bool {
        ["ssh-", "ecdsa-sha2-", "sk-ssh-", "sk-ecdsa-"].iter().any(|p| t.starts_with(p))
    }
    let line = line.trim();
    let first = line.split_whitespace().next()?;
    let (options, rest) = if key_type(first) {
        (Vec::new(), line)
    } else {
        // The options field ends at the first whitespace outside quotes.
        let (mut quoted, mut escaped, mut end) = (false, false, line.len());
        for (i, c) in line.char_indices() {
            match c {
                _ if escaped => escaped = false,
                '\\' if quoted => escaped = true,
                '"' => quoted = !quoted,
                c if c.is_whitespace() && !quoted => {
                    end = i;
                    break;
                }
                _ => {}
            }
        }
        let field = &line[..end];
        let mut names = Vec::new();
        let (mut quoted, mut escaped, mut start) = (false, false, 0);
        for (i, c) in field.char_indices().chain([(field.len(), ',')]) {
            match c {
                _ if escaped => escaped = false,
                '\\' if quoted => escaped = true,
                '"' => quoted = !quoted,
                ',' if !quoted => {
                    let opt = &field[start..i];
                    let name = opt.split('=').next().unwrap_or("").trim().to_ascii_lowercase();
                    if !name.is_empty() {
                        names.push(name);
                    }
                    start = i + 1;
                }
                _ => {}
            }
        }
        (names, line[end..].trim_start())
    };
    let mut parts = rest.split_whitespace();
    let _type = parts.next()?;
    let key = russh_keys::parse_public_key_base64(parts.next()?).ok()?;
    Some((options, key))
}

/// Whether an OpenSSH `authorized_keys` text lets `key` in: a line `[options]
/// type base64 [comment]` naming it whose options are all ones stormd
/// enforces (`ENFORCED_OPTIONS`). A line naming it with any other option is
/// refused with a WARN naming them — its restriction would silently not
/// apply (stormd#86). Blank lines, comments and lines with no key are
/// skipped.
pub fn authorized_keys_contains(text: &str, key: &PublicKey) -> bool {
    for (n, line) in text.lines().enumerate() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let Some((options, k)) = parse_authorized_key(l) else { continue };
        if &k != key {
            continue;
        }
        let refused: Vec<&String> = options.iter().filter(|o| !ENFORCED_OPTIONS.contains(&o.as_str())).collect();
        if refused.is_empty() {
            return true;
        }
        warn!(
            line = n + 1, options = ?refused,
            "authorized_keys entry restricts this key with options stormd does not enforce — refused"
        );
    }
    false
}

#[async_trait]
impl Handler for SshSession {
    type Error = anyhow::Error;

    async fn auth_password(
        &mut self,
        _user: &str,
        password: &str,
    ) -> Result<Auth, Self::Error> {
        if password == self.config.password || password == self.app_state.cloud_id {
            Ok(Auth::Accept)
        } else {
            let methods = if self.has_keys() {
                MethodSet::PUBLICKEY | MethodSet::PASSWORD
            } else {
                MethodSet::PASSWORD
            };
            Ok(Auth::Reject {
                proceed_with_methods: Some(methods),
            })
        }
    }

    async fn auth_publickey_offered(
        &mut self,
        _user: &str,
        public_key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        if self.in_authorized_keys(public_key) {
            return Ok(Auth::Accept);
        }
        if let Some(ref store) = self.cloudid_keys {
            let keys = store.read().await;
            if keys.contains(public_key) {
                return Ok(Auth::Accept);
            }
        }
        Ok(Auth::Reject {
            proceed_with_methods: Some(MethodSet::PUBLICKEY | MethodSet::PASSWORD),
        })
    }

    async fn auth_publickey(
        &mut self,
        user: &str,
        public_key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        if self.in_authorized_keys(public_key) {
            info!(user = user, "SSH public key auth accepted (authorized_keys)");
            return Ok(Auth::Accept);
        }
        if let Some(ref store) = self.cloudid_keys {
            let keys = store.read().await;
            if keys.contains(public_key) {
                if let Some(owner) = keys.lookup(public_key) {
                    info!(user = user, owner = owner, "SSH public key auth accepted (CloudID)");
                }
                return Ok(Auth::Accept);
            }
        }
        Ok(Auth::Reject {
            proceed_with_methods: Some(MethodSet::PASSWORD),
        })
    }

    async fn auth_none(&mut self, _user: &str) -> Result<Auth, Self::Error> {
        let methods = if self.has_keys() {
            MethodSet::PUBLICKEY | MethodSet::PASSWORD
        } else {
            MethodSet::PASSWORD
        };
        Ok(Auth::Reject {
            proceed_with_methods: Some(methods),
        })
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        let id = channel.id();
        {
            let mut channels = self.channels.lock().await;
            channels.insert(
                id,
                ChannelState {
                    line_buffer: String::new(),
                    history: Vec::new(),
                },
            );
        }
        {
            let mut raw = self.raw_channels.lock().await;
            raw.insert(id, channel);
        }
        Ok(true)
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        _col_width: u32,
        _row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let banner = format!(
            "\r\n\x1b[1mstormd\x1b[0m -- container management shell\r\n\
             Container: {}\r\n\
             Type \x1b[1mhelp\x1b[0m for available commands.\r\n\r\n",
            self.container_name
        );
        session.data(channel, CryptoVec::from(banner.into_bytes()));
        self.send_prompt(session, channel);
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let banner = format!(
            "\r\nstormd -- container management shell\r\n\
             Container: {}\r\n\
             Type help for available commands.\r\n\r\n",
            self.container_name
        );
        session.data(channel, CryptoVec::from(banner.into_bytes()));
        self.send_prompt(session, channel);
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let mut channels = self.channels.lock().await;
        let state = match channels.get_mut(&channel) {
            Some(s) => s,
            None => return Ok(()),
        };

        for &byte in data {
            match byte {
                // Enter
                b'\r' | b'\n' => {
                    session.data(channel, cv(b"\r\n"));
                    let line = state.line_buffer.clone();
                    if !line.trim().is_empty() {
                        state.history.push(line.clone());
                    }
                    state.line_buffer.clear();

                    let output = shell::execute_command(
                        &line,
                        &self.app_state,
                        &self.container_name,
                        self.started_at,
                    )
                    .await;

                    session.data(channel, CryptoVec::from(output.text.into_bytes()));

                    if output.exit {
                        session.close(channel);
                        return Ok(());
                    }

                    drop(channels);
                    self.send_prompt(session, channel);
                    return Ok(());
                }
                // Backspace
                // Not a guard on the arm: 0x7f on an empty line must stop
                // here, not fall through to the printable arm below.
                0x7f | 0x08 => {
                    let erased = state.line_buffer.pop().is_some();
                    if erased {
                        session.data(channel, cv(b"\x08 \x08"));
                    }
                }
                // Ctrl-C
                0x03 => {
                    state.line_buffer.clear();
                    session.data(channel, cv(b"^C\r\n"));
                    drop(channels);
                    self.send_prompt(session, channel);
                    return Ok(());
                }
                // Ctrl-D (EOF)
                0x04 if state.line_buffer.is_empty() => {
                    session.data(channel, cv(b"\r\nlogout\r\n"));
                    session.close(channel);
                    return Ok(());
                }
                // Tab completion
                0x09 => {
                    let partial = state.line_buffer.clone();
                    let completions = shell::complete(&partial, &self.app_state).await;
                    if completions.len() == 1 {
                        let parts: Vec<&str> = partial.split_whitespace().collect();
                        let last_word = parts.last().copied().unwrap_or("");
                        let completion = &completions[0];
                        let suffix = &completion[last_word.len()..];
                        state.line_buffer.push_str(suffix);
                        state.line_buffer.push(' ');
                        let echo = format!("{} ", suffix);
                        session.data(channel, CryptoVec::from(echo.into_bytes()));
                    } else if completions.len() > 1 {
                        let list = completions.join("  ");
                        let msg = format!("\r\n{}\r\n", list);
                        session.data(channel, CryptoVec::from(msg.into_bytes()));
                        let line = state.line_buffer.clone();
                        drop(channels);
                        self.send_prompt(session, channel);
                        session.data(channel, CryptoVec::from(line.into_bytes()));
                        return Ok(());
                    }
                }
                // Regular printable character
                b if b >= 0x20 => {
                    state.line_buffer.push(byte as char);
                    session.data(channel, cv(&[byte]));
                }
                _ => {}
            }
        }

        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let mut channels = self.channels.lock().await;
        channels.remove(&channel);
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name == "sftp" {
            let channel = {
                let mut raw = self.raw_channels.lock().await;
                raw.remove(&channel_id)
            };
            if let Some(channel) = channel {
                info!("SFTP subsystem requested");
                session.channel_success(channel_id);
                let sftp = crate::sftp::SftpSession::default();
                russh_sftp::server::run(channel.into_stream(), sftp).await;
            } else {
                warn!("SFTP requested but channel not found");
                session.channel_failure(channel_id);
            }
        } else {
            session.channel_failure(channel_id);
        }
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.close(channel);
        Ok(())
    }
}

#[cfg(test)]
mod authorized_keys_tests {
    use super::authorized_keys_contains;
    use russh_keys::PublicKeyBase64;

    #[test]
    fn lines_comments_and_options() {
        let key = russh_keys::key::KeyPair::generate_ed25519().clone_public_key().unwrap();
        let other = russh_keys::key::KeyPair::generate_ed25519().clone_public_key().unwrap();
        let b64 = key.public_key_base64();
        let text = format!("# admins\n\nssh-ed25519 {b64} ops@host\n");
        assert!(authorized_keys_contains(&text, &key));
        assert!(!authorized_keys_contains(&text, &other), "a key not listed");
        let enforced = format!("no-port-forwarding,no-agent-forwarding,no-X11-forwarding,no-user-rc ssh-ed25519 {b64} x\n");
        assert!(authorized_keys_contains(&enforced, &key), "options stormd holds by construction");
        assert!(!authorized_keys_contains("# nothing\nnot a key\n", &key));
    }

    /// stormd#86 (94c19c75c80b): a restriction stormd would not enforce
    /// refuses the entry instead of vanishing.
    #[test]
    fn unenforced_restrictions_refuse_the_entry() {
        let key = russh_keys::key::KeyPair::generate_ed25519().clone_public_key().unwrap();
        let b64 = key.public_key_base64();
        for opts in [
            "from=\"10.0.0.0/8\"",
            "command=\"/usr/bin/backup --only, this\"",
            "restrict",
            "no-pty",
            "no-port-forwarding,environment=\"A=b c\"",
            "expiry-time=\"20990101\"",
            "cert-authority",
        ] {
            let text = format!("{opts} ssh-ed25519 {b64} ops@host\n");
            assert!(!authorized_keys_contains(&text, &key), "accepted despite {opts}");
        }
        // The same key unrestricted on another line still gets in.
        let both = format!("restrict ssh-ed25519 {b64} a\nssh-ed25519 {b64} b\n");
        assert!(authorized_keys_contains(&both, &key));
    }

    #[test]
    fn quoted_options_are_parsed_whole() {
        let key = russh_keys::key::KeyPair::generate_ed25519().clone_public_key().unwrap();
        let b64 = key.public_key_base64();
        let line = format!("command=\"echo \\\"a, b\\\" c\",no-pty ssh-ed25519 {b64} x");
        let (opts, k) = super::parse_authorized_key(&line).unwrap();
        assert_eq!(opts, vec!["command", "no-pty"]);
        assert_eq!(k, key);
        let (opts, _) = super::parse_authorized_key(&format!("ssh-ed25519 {b64}")).unwrap();
        assert!(opts.is_empty());
    }
}

/// stormd#86 (813579212408): the host key is the owner's alone.
#[cfg(test)]
mod host_key_tests {
    use super::load_or_generate_host_key;
    use russh_keys::PublicKeyBase64;
    use std::os::unix::fs::PermissionsExt;

    fn dir(label: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("stormd-hostkey-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn mode(p: &std::path::Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[tokio::test]
    async fn a_new_key_is_0600_and_loads_again() {
        let d = dir("new");
        let path = d.join("etc/host_key");
        let old = unsafe { libc::umask(0o022) };
        let made = load_or_generate_host_key(&path).await;
        unsafe { libc::umask(old) };
        let m = mode(&path);
        let again = load_or_generate_host_key(&path).await;
        let tmp_left = d.join("etc/host_key.tmp").exists();
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(m, 0o600, "saved {m:o}");
        assert_eq!(
            again.clone_public_key().unwrap().public_key_base64(),
            made.clone_public_key().unwrap().public_key_base64(),
            "the saved key is loaded, not replaced"
        );
        assert!(!tmp_left);
    }

    #[tokio::test]
    async fn an_existing_key_readable_by_others_is_tightened() {
        let d = dir("loose");
        let path = d.join("host_key");
        let made = load_or_generate_host_key(&path).await;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let loaded = load_or_generate_host_key(&path).await;
        let m = mode(&path);
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(m, 0o600);
        assert_eq!(
            loaded.clone_public_key().unwrap().public_key_base64(),
            made.clone_public_key().unwrap().public_key_base64()
        );
    }

    #[tokio::test]
    async fn a_symlink_is_neither_followed_nor_replaced() {
        let d = dir("link");
        let target = d.join("elsewhere");
        std::fs::write(&target, "not a key").unwrap();
        let path = d.join("host_key");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let _key = load_or_generate_host_key(&path).await;
        let still_link = std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink();
        let target_text = std::fs::read_to_string(&target).unwrap();
        let _ = std::fs::remove_dir_all(&d);
        assert!(still_link, "the symlink was replaced");
        assert_eq!(target_text, "not a key", "written through the symlink");
    }
}
