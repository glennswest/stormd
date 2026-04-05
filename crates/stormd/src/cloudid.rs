use russh_keys::key::PublicKey;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Authorized SSH public keys fetched from a CloudID metadata endpoint.
#[derive(Clone, Default)]
pub struct SshKeyStore {
    keys: Vec<(PublicKey, String)>,
}

impl SshKeyStore {
    /// Check whether a public key is authorized.
    pub fn contains(&self, key: &PublicKey) -> bool {
        self.keys.iter().any(|(k, _)| k == key)
    }

    /// Look up the username associated with a public key.
    pub fn lookup(&self, key: &PublicKey) -> Option<&str> {
        self.keys
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, user)| user.as_str())
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }
}

/// Fetch SSH public keys from a CloudID (EC2-compatible) metadata endpoint.
///
/// The index endpoint (`/latest/meta-data/public-keys/`) returns lines like
/// `"0=gwest\n1=root\n"`. Each entry is fetched individually for its
/// OpenSSH key(s) at `/latest/meta-data/public-keys/{idx}/openssh-key`.
async fn fetch_keys(client: &reqwest::Client, base_url: &str) -> Result<SshKeyStore, String> {
    let index_url = format!(
        "{}/latest/meta-data/public-keys/",
        base_url.trim_end_matches('/')
    );
    let index_text = client
        .get(&index_url)
        .send()
        .await
        .map_err(|e| format!("CloudID index fetch failed: {e}"))?
        .text()
        .await
        .map_err(|e| format!("CloudID index read failed: {e}"))?;

    let mut keys = Vec::new();

    for line in index_text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Format: "0=gwest" or "1=root"
        let (idx, username) = match line.split_once('=') {
            Some(pair) => pair,
            None => continue,
        };

        let key_url = format!(
            "{}/latest/meta-data/public-keys/{}/openssh-key",
            base_url.trim_end_matches('/'),
            idx
        );

        let key_text = match client.get(&key_url).send().await {
            Ok(resp) => match resp.text().await {
                Ok(t) => t,
                Err(e) => {
                    warn!("CloudID key read failed for {username}: {e}");
                    continue;
                }
            },
            Err(e) => {
                warn!("CloudID key fetch failed for {username}: {e}");
                continue;
            }
        };

        // Each response may contain multiple keys (one per line)
        for key_line in key_text.lines() {
            let key_line = key_line.trim();
            if key_line.is_empty() || key_line.starts_with('#') {
                continue;
            }
            // OpenSSH format: "ssh-ed25519 AAAA... comment"
            let base64_part = match key_line.split_whitespace().nth(1) {
                Some(b) => b,
                None => {
                    // Try as raw base64
                    key_line
                }
            };
            match russh_keys::parse_public_key_base64(base64_part) {
                Ok(pk) => {
                    debug!(
                        user = username,
                        algo = pk.name(),
                        fingerprint = %pk.fingerprint(),
                        "loaded SSH key from CloudID"
                    );
                    keys.push((pk, username.to_string()));
                }
                Err(e) => {
                    warn!("failed to parse SSH key for {username}: {e}");
                }
            }
        }
    }

    info!(count = keys.len(), "CloudID: loaded SSH keys");
    Ok(SshKeyStore { keys })
}

/// Start the CloudID key refresh loop.
///
/// Fetches keys immediately, then refreshes every 30 seconds.
/// If the initial fetch fails, starts with an empty store and retries.
pub async fn start_key_refresh(
    cloudid_url: String,
) -> Arc<RwLock<SshKeyStore>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    // Try initial fetch
    let initial = match fetch_keys(&client, &cloudid_url).await {
        Ok(store) => store,
        Err(e) => {
            warn!("CloudID initial fetch failed (will retry): {e}");
            SshKeyStore::default()
        }
    };

    let store = Arc::new(RwLock::new(initial));
    let bg_store = Arc::clone(&store);

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        interval.tick().await; // skip first immediate tick
        loop {
            interval.tick().await;
            match fetch_keys(&client, &cloudid_url).await {
                Ok(new_store) => {
                    let count = new_store.len();
                    *bg_store.write().await = new_store;
                    debug!(count, "CloudID keys refreshed");
                }
                Err(e) => {
                    warn!("CloudID refresh failed (keeping old keys): {e}");
                }
            }
        }
    });

    store
}
