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

async fn load_or_generate_host_key(path: &std::path::Path) -> KeyPair {
    // Try to load existing key
    if path.exists() {
        match russh_keys::load_secret_key(path, None) {
            Ok(key) => {
                info!(path = %path.display(), "loaded SSH host key");
                return key;
            }
            Err(e) => {
                warn!(error = %e, "failed to load host key, generating new one");
            }
        }
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
    } else if let Err(e) = tokio::fs::write(path, &buf).await {
        warn!(error = %e, path = %path.display(), "failed to save host key");
    } else {
        info!(path = %path.display(), "generated new SSH host key");
    }

    key
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
            let methods = if self.cloudid_keys.is_some() {
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
        let methods = if self.cloudid_keys.is_some() {
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
                0x7f | 0x08 => {
                    if !state.line_buffer.is_empty() {
                        state.line_buffer.pop();
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
                0x04 => {
                    if state.line_buffer.is_empty() {
                        session.data(channel, cv(b"\r\nlogout\r\n"));
                        session.close(channel);
                        return Ok(());
                    }
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
