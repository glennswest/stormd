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

/// How long a metadata token is asked for — IMDSv2's usual six hours, and
/// stormimds's `token_max_ttl_seconds` default.
const TOKEN_TTL_SECS: u64 = 21600;

/// A client for an EC2-compatible metadata service, speaking IMDSv2.
///
/// **Every request was a bare GET** (stormd#19). stormimds, the metadata
/// service on every stormcos node, answers that `401` with an empty body in
/// its default mode, and the empty body parsed as an index with no entries:
/// no keys, and nothing said. So this does what IMDSv2 defines — `PUT
/// /latest/api/token`, then the token on every GET — and falls back to no
/// token only when the service refuses to issue one (IMDSv1, and services
/// that never had tokens). Every request also carries `Metadata-Flavor:
/// StormIMDS`, the other credential stormimds accepts (its `header` mode
/// takes nothing else); EC2 ignores it. A status that is not 2xx is an error
/// with the status in it, never a body to parse.
pub(crate) struct Imds {
    client: reqwest::Client,
    base: String,
    /// The token and when to stop using it (a minute before it expires).
    token: Option<(String, std::time::Instant)>,
}

impl Imds {
    pub(crate) fn new(client: reqwest::Client, base_url: &str) -> Imds {
        Imds { client, base: base_url.trim_end_matches('/').to_string(), token: None }
    }

    /// A current token, asking for one if there is none; `None` when the
    /// service does not issue them.
    async fn token(&mut self) -> Option<String> {
        if let Some((t, until)) = &self.token {
            if std::time::Instant::now() < *until {
                return Some(t.clone());
            }
        }
        self.token = None;
        let resp = self
            .client
            .put(format!("{}/latest/api/token", self.base))
            .header("X-aws-ec2-metadata-token-ttl-seconds", TOKEN_TTL_SECS.to_string())
            .header("Metadata-Flavor", "StormIMDS")
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() => {
                let t = r.text().await.unwrap_or_default().trim().to_string();
                if t.is_empty() {
                    return None;
                }
                let until = std::time::Instant::now() + std::time::Duration::from_secs(TOKEN_TTL_SECS - 60);
                self.token = Some((t.clone(), until));
                Some(t)
            }
            Ok(r) => {
                debug!(status = %r.status(), "metadata service issues no token — using none");
                None
            }
            Err(e) => {
                debug!(error = %e, "metadata token request failed — using none");
                None
            }
        }
    }

    /// GET `path` (relative to the base URL), requiring 2xx. A `401` while
    /// holding a token means the token went stale (the service restarted, or
    /// it expired early): fetch a new one and try once more.
    pub(crate) async fn get(&mut self, path: &str) -> Result<String, String> {
        let url = format!("{}/{}", self.base, path.trim_start_matches('/'));
        for attempt in 0..2 {
            let token = self.token().await;
            let mut req = self.client.get(&url).header("Metadata-Flavor", "StormIMDS");
            if let Some(t) = &token {
                req = req.header("X-aws-ec2-metadata-token", t);
            }
            let resp = req.send().await.map_err(|e| format!("GET {url}: {e}"))?;
            let status = resp.status();
            if status == reqwest::StatusCode::UNAUTHORIZED && token.is_some() && attempt == 0 {
                self.token = None;
                continue;
            }
            if !status.is_success() {
                return Err(format!("GET {url}: HTTP {status}"));
            }
            return resp.text().await.map_err(|e| format!("GET {url}: {e}"));
        }
        unreachable!("the loop returns on its second attempt")
    }
}

/// Fetch SSH public keys from a CloudID (EC2-compatible) metadata endpoint.
///
/// The index endpoint (`/latest/meta-data/public-keys/`) returns lines like
/// `"0=gwest\n1=root\n"`. Each entry is fetched individually for its
/// OpenSSH key(s) at `/latest/meta-data/public-keys/{idx}/openssh-key`.
pub(crate) async fn fetch_keys(imds: &mut Imds) -> Result<SshKeyStore, String> {
    let index_text = imds
        .get("latest/meta-data/public-keys/")
        .await
        .map_err(|e| format!("CloudID index: {e}"))?;

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

        let key_text = match imds.get(&format!("latest/meta-data/public-keys/{idx}/openssh-key")).await {
            Ok(t) => t,
            Err(e) => {
                warn!("CloudID key for {username}: {e}");
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
/// A failure is warned about once, and again only when it changes; the
/// recovery is logged too — not the same line every 30 s for as long as the
/// service is down.
pub async fn start_key_refresh(
    cloudid_url: String,
) -> Arc<RwLock<SshKeyStore>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();
    let mut imds = Imds::new(client, &cloudid_url);

    // Try initial fetch
    let mut last_error = None;
    let initial = match fetch_keys(&mut imds).await {
        Ok(store) => store,
        Err(e) => {
            warn!("CloudID initial fetch failed (will retry): {e}");
            last_error = Some(e);
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
            match fetch_keys(&mut imds).await {
                Ok(new_store) => {
                    let count = new_store.len();
                    *bg_store.write().await = new_store;
                    if last_error.take().is_some() {
                        info!(count, "CloudID refresh recovered");
                    }
                    debug!(count, "CloudID keys refreshed");
                }
                Err(e) => {
                    if last_error.as_ref() != Some(&e) {
                        warn!("CloudID refresh failed (keeping old keys): {e}");
                    } else {
                        debug!("CloudID refresh still failing: {e}");
                    }
                    last_error = Some(e);
                }
            }
        }
    });

    store
}

#[cfg(test)]
mod tests {
    //! Against a stand-in for stormimds: the same token endpoint, headers and
    //! security modes (stormimds `src/auth/middleware.rs`, `src/api/token.rs`).

    use super::{fetch_keys, Imds};
    use axum::{
        extract::{Path, State},
        http::{HeaderMap, StatusCode},
        routing::{get, put},
        Router,
    };
    use russh_keys::PublicKeyBase64;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Copy, PartialEq)]
    enum Mode {
        /// stormimds `both`: a token or the flavor header.
        Both,
        /// stormimds `token`: only a token.
        Token,
        /// stormimds `header`: only the flavor header.
        Header,
        /// An IMDSv1 service: no token endpoint, no checks.
        V1,
        /// Refuses every metadata request.
        Deny,
    }

    #[derive(Default)]
    struct Seen {
        tokens_issued: u32,
        valid: Vec<String>,
        /// Forget every issued token after this many metadata GETs (a
        /// service restart, as far as the client can tell).
        forget_after: Option<u32>,
        gets: u32,
    }

    #[derive(Clone)]
    struct Stand {
        mode: Mode,
        keys: Arc<Vec<String>>,
        seen: Arc<Mutex<Seen>>,
    }

    fn authorized(st: &Stand, h: &HeaderMap) -> bool {
        let mut seen = st.seen.lock().unwrap();
        seen.gets += 1;
        if seen.forget_after == Some(seen.gets) {
            seen.valid.clear();
        }
        let token_ok = h
            .get("x-aws-ec2-metadata-token")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|t| seen.valid.iter().any(|v| v == t));
        let flavor_ok = h.get("metadata-flavor").and_then(|v| v.to_str().ok()) == Some("StormIMDS");
        match st.mode {
            Mode::Both => token_ok || flavor_ok,
            Mode::Token => token_ok,
            Mode::Header => flavor_ok,
            Mode::V1 => true,
            Mode::Deny => false,
        }
    }

    async fn serve(mode: Mode, forget_after: Option<u32>) -> (String, Stand) {
        let pairs = [russh_keys::key::KeyPair::generate_ed25519(), russh_keys::key::KeyPair::generate_ed25519()];
        let keys: Vec<String> = pairs
            .iter()
            .map(|k| format!("ssh-ed25519 {} test", k.clone_public_key().unwrap().public_key_base64()))
            .collect();
        let st = Stand {
            mode,
            keys: Arc::new(keys),
            seen: Arc::new(Mutex::new(Seen { forget_after, ..Default::default() })),
        };
        let mut app = Router::new()
            .route(
                "/latest/meta-data/public-keys/",
                get(|State(st): State<Stand>, h: HeaderMap| async move {
                    if !authorized(&st, &h) {
                        return Err(StatusCode::UNAUTHORIZED);
                    }
                    Ok("0=gwest\n1=root\n".to_string())
                }),
            )
            .route(
                "/latest/meta-data/public-keys/{idx}/openssh-key",
                get(|State(st): State<Stand>, Path(idx): Path<usize>, h: HeaderMap| async move {
                    if !authorized(&st, &h) {
                        return Err(StatusCode::UNAUTHORIZED);
                    }
                    st.keys.get(idx).cloned().ok_or(StatusCode::NOT_FOUND)
                }),
            );
        if mode != Mode::V1 {
            app = app.route(
                "/latest/api/token",
                put(|State(st): State<Stand>, h: HeaderMap| async move {
                    let ttl: u64 = h
                        .get("x-aws-ec2-metadata-token-ttl-seconds")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse().ok())
                        .ok_or(StatusCode::BAD_REQUEST)?;
                    if ttl == 0 || ttl > 21600 {
                        return Err(StatusCode::BAD_REQUEST);
                    }
                    let mut seen = st.seen.lock().unwrap();
                    seen.tokens_issued += 1;
                    let t = format!("tok-{}", seen.tokens_issued);
                    seen.valid.push(t.clone());
                    Ok(t)
                }),
            );
        }
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let app = app.with_state(st.clone());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (url, st)
    }

    async fn keys(url: &str) -> (Result<usize, String>, Imds) {
        let mut imds = Imds::new(reqwest::Client::new(), url);
        let r = fetch_keys(&mut imds).await.map(|s| s.len());
        (r, imds)
    }

    #[tokio::test]
    async fn stormimds_modes_all_yield_the_keys() {
        for mode in [Mode::Both, Mode::Token, Mode::Header] {
            let (url, st) = serve(mode, None).await;
            let (r, _) = keys(&url).await;
            assert_eq!(r, Ok(2), "mode {}", mode as u8);
            // One token for the whole refresh, not one per request.
            assert_eq!(st.seen.lock().unwrap().tokens_issued, 1);
        }
    }

    #[tokio::test]
    async fn a_service_without_tokens_still_works() {
        let (url, _) = serve(Mode::V1, None).await;
        assert_eq!(keys(&url).await.0, Ok(2));
    }

    #[tokio::test]
    async fn a_refusal_is_an_error_with_its_status_not_an_empty_index() {
        let (url, _) = serve(Mode::Deny, None).await;
        let e = keys(&url).await.0.unwrap_err();
        assert!(e.contains("401"), "{e}");
    }

    #[tokio::test]
    async fn a_token_the_service_forgot_is_replaced_once() {
        // The service forgets its tokens on the second GET (the first key).
        let (url, st) = serve(Mode::Token, Some(2)).await;
        let (r, mut imds) = keys(&url).await;
        assert_eq!(r, Ok(2));
        assert_eq!(st.seen.lock().unwrap().tokens_issued, 2);
        // And the new token is kept for the next refresh.
        assert_eq!(fetch_keys(&mut imds).await.map(|s| s.len()), Ok(2));
        assert_eq!(st.seen.lock().unwrap().tokens_issued, 2);
    }
}
