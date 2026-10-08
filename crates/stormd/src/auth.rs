//! Authentication hooks for the API and UI. Off unless `[api]` configures
//! `[[api.users]]` (name + password), a legacy `password` (the "admin"
//! user), `auth_token` / `token_file` (machine bearer token) or
//! `client_ca_file` (client certificates, over TLS) — then everything except
//! the health checks, the auth endpoints and the static UI assets requires a
//! verified client certificate, a session cookie or a bearer token.
//!
//! Sessions live in memory: a restart signs everyone out, which for a
//! container's init is the right default. Anything longer-lived (users,
//! external identity, persistence) belongs behind these same three
//! endpoints and this middleware — that is the extension point.

use crate::api::AppState;
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

const SESSION_TTL: Duration = Duration::from_secs(24 * 3600);
const COOKIE: &str = "stormd_session";

struct Session {
    user: String,
    created: Instant,
}

pub struct AuthState {
    /// (name, password) pairs — [[api.users]], plus legacy `password` as
    /// the "admin" user.
    users: Vec<(String, String)>,
    token: Option<String>,
    token_file: Option<TokenFile>,
    sessions: RwLock<HashMap<String, Session>>,
}

/// Request extension the TLS listener sets when the connection presented a
/// client certificate that verified against `[api] client_ca_file`.
#[derive(Clone, Copy, Debug)]
pub struct ClientCertVerified;

/// A bearer token kept in a file, re-read whenever the file's modification
/// time or size changes — a rotated token works without restarting stormd.
/// An unreadable or empty file means no token (nothing matches), not an open
/// API.
struct TokenFile {
    path: PathBuf,
    cached: Mutex<(Option<(std::time::SystemTime, u64)>, Option<String>)>,
}

impl TokenFile {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            cached: Mutex::new((None, None)),
        }
    }

    fn current(&self) -> Option<String> {
        let stamp = std::fs::metadata(&self.path)
            .ok()
            .and_then(|m| Some((m.modified().ok()?, m.len())));
        let mut cached = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        if stamp.is_none() || cached.0 != stamp {
            let token = std::fs::read_to_string(&self.path)
                .ok()
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty());
            if token.is_none() {
                tracing::warn!(path = %self.path.display(), "[api] token_file unreadable or empty — no bearer token accepted from it");
            }
            *cached = (stamp, token);
        }
        cached.1.clone()
    }
}

impl AuthState {
    /// None when no credential is configured — auth disabled, everything open.
    pub fn from_config(api: &crate::config::ApiConfig) -> Option<Arc<Self>> {
        let mut users: Vec<(String, String)> = api
            .users
            .iter()
            .map(|u| (u.name.clone(), u.password.clone()))
            .collect();
        if let Some(p) = &api.password {
            users.push(("admin".to_string(), p.clone()));
        }
        if users.is_empty()
            && api.auth_token.is_none()
            && api.token_file.is_none()
            && api.client_ca_file.is_none()
        {
            return None;
        }
        Some(Arc::new(Self {
            users,
            token: api.auth_token.clone(),
            token_file: api.token_file.clone().map(TokenFile::new),
            sessions: RwLock::new(HashMap::new()),
        }))
    }

    /// The user whose credentials these are, if any. Every configured pair
    /// is compared — never an early exit on a name match — so timing says
    /// nothing about which usernames exist. The bearer token doubles as the
    /// "admin" login, so a machine credential also opens the UI.
    fn check_credentials(&self, username: &str, password: &str) -> Option<String> {
        let mut matched: Option<String> = None;
        for (name, expect) in &self.users {
            let name_ok = ct_eq(username, name);
            let pass_ok = ct_eq(password, expect);
            if name_ok && pass_ok {
                matched = Some(name.clone());
            }
        }
        if matched.is_none() {
            let admin = username.is_empty() || username == "admin";
            if admin && self.token_matches(password) {
                matched = Some("admin".to_string());
            }
        }
        matched
    }

    /// stormd's machine bearer tokens: `auth_token` and the current contents
    /// of `token_file`, whichever are configured.
    pub fn tokens(&self) -> Vec<String> {
        let mut out: Vec<String> = self.token.iter().cloned().collect();
        if let Some(t) = self.token_file.as_ref().and_then(TokenFile::current) {
            out.push(t);
        }
        out
    }

    /// Every token is compared, so timing says nothing about which matched.
    fn token_matches(&self, given: &str) -> bool {
        self.tokens()
            .iter()
            .fold(false, |ok, t| ct_eq(given, t) | ok)
    }

    async fn new_session(&self, user: &str) -> String {
        let id = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let mut sessions = self.sessions.write().await;
        sessions.retain(|_, s| s.created.elapsed() < SESSION_TTL);
        sessions.insert(
            id.clone(),
            Session {
                user: user.to_string(),
                created: Instant::now(),
            },
        );
        id
    }

    /// The session's user, if the session exists and is fresh.
    async fn session_user(&self, id: &str) -> Option<String> {
        let sessions = self.sessions.read().await;
        sessions
            .get(id)
            .filter(|s| s.created.elapsed() < SESSION_TTL)
            .map(|s| s.user.clone())
    }

    async fn session_valid(&self, id: &str) -> bool {
        self.session_user(id).await.is_some()
    }

    async fn drop_session(&self, id: &str) {
        self.sessions.write().await.remove(id);
    }
}

/// Constant-time string comparison — an attacker timing login failures learns
/// nothing about how much of the guess matched.
fn ct_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= (x ^ y) as usize;
    }
    diff == 0
}

/// Paths that stay open even with auth on: liveness for orchestrators, the
/// auth endpoints themselves, and the static SPA (which shows the login
/// screen — the data behind it is what's protected). `/metrics` is NOT
/// public (stormd#32): it names every process, so a scraper sends the bearer
/// token or a client certificate like any other caller. Nor is the plugin
/// proxy: it reaches into other processes.
fn is_public(path: &str) -> bool {
    path == "/"
        || path == "/healthz"
        || path == "/api/v1/health"
        || path.starts_with("/api/v1/auth/")
        || (path.starts_with("/ui/") && !path.starts_with("/ui/proxy/"))
}

/// Whether a `name=value` cookie pair (or a `Set-Cookie` value) is
/// stormd's own session cookie.
pub fn is_session_cookie(pair: &str) -> bool {
    pair.trim_start()
        .split_once('=')
        .map(|(name, _)| name.trim() == COOKIE)
        .unwrap_or(false)
}

fn session_cookie(req: &Request) -> Option<String> {
    let cookies = req.headers().get(header::COOKIE)?.to_str().ok()?;
    for part in cookies.split(';') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix(&format!("{}=", COOKIE)) {
            return Some(v.to_string());
        }
    }
    None
}

pub async fn require_auth(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let Some(auth) = &state.auth else {
        return next.run(req).await;
    };
    if is_public(req.uri().path()) {
        return next.run(req).await;
    }

    // A client certificate the TLS listener verified against client_ca_file.
    if req.extensions().get::<ClientCertVerified>().is_some() {
        return next.run(req).await;
    }

    if let Some(given) = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        if auth.token_matches(given) {
            return next.run(req).await;
        }
    }

    // Browser sessions — the cookie also rides along on WebSocket upgrades.
    if let Some(id) = session_cookie(&req) {
        if auth.session_valid(&id).await {
            return next.run(req).await;
        }
    }

    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "error": "authentication required" })),
    )
        .into_response()
}

// --- Endpoints ---

#[derive(Deserialize)]
pub struct LoginRequest {
    #[serde(default)]
    username: String,
    password: String,
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LoginRequest>,
) -> Response {
    let Some(auth) = &state.auth else {
        return Json(serde_json::json!({ "ok": true, "required": false })).into_response();
    };
    let Some(user) = auth.check_credentials(&body.username, &body.password) else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "wrong user or password" })),
        )
            .into_response();
    };
    let id = auth.new_session(&user).await;
    (
        [(
            header::SET_COOKIE,
            format!(
                "{}={}; HttpOnly; Path=/; SameSite=Lax; Max-Age={}",
                COOKIE,
                id,
                SESSION_TTL.as_secs()
            ),
        )],
        Json(serde_json::json!({ "ok": true, "user": user })),
    )
        .into_response()
}

pub async fn logout(State(state): State<Arc<AppState>>, req: Request) -> Response {
    if let (Some(auth), Some(id)) = (&state.auth, session_cookie(&req)) {
        auth.drop_session(&id).await;
    }
    (
        [(
            header::SET_COOKIE,
            format!("{}=; HttpOnly; Path=/; SameSite=Lax; Max-Age=0", COOKIE),
        )],
        Json(serde_json::json!({ "ok": true })),
    )
        .into_response()
}

/// Always open: the UI asks this first to decide whether to show the login
/// screen at all. It also carries what the login screen itself needs — the
/// instance name and the configured default theme — since everything else
/// is behind the gate at that point.
pub async fn session(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let user = match &state.auth {
        None => None,
        Some(auth) => match session_cookie(&req) {
            Some(id) => auth.session_user(&id).await,
            None => None,
        },
    };
    let authenticated = state.auth.is_none() || user.is_some();
    Json(serde_json::json!({
        "required": state.auth.is_some(),
        "authenticated": authenticated,
        "user": user,
        "container": state.container_name,
        "theme": state.ui_theme,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_compares_correctly() {
        assert!(ct_eq("secret", "secret"));
        assert!(!ct_eq("secret", "secre"));
        assert!(!ct_eq("secret", "secrex"));
        assert!(!ct_eq("", "x"));
        assert!(ct_eq("", ""));
    }

    #[test]
    fn public_paths() {
        assert!(is_public("/api/v1/health"));
        assert!(is_public("/healthz"));
        assert!(!is_public("/api/v1/health/apis"), "API health is data, behind auth (stormd#49)");
        assert!(!is_public("/metrics"));
        assert!(is_public("/api/v1/auth/login"));
        assert!(is_public("/ui/"));
        assert!(is_public("/ui/assets/app.js"));
        assert!(!is_public("/ui/proxy/myapp/"));
        assert!(!is_public("/api/v1/processes"));
        assert!(!is_public("/ws/logs"));
    }

    fn api(toml_text: &str) -> crate::config::ApiConfig {
        toml::from_str(toml_text).unwrap()
    }

    #[test]
    fn token_file_and_client_ca_turn_auth_on() {
        assert!(AuthState::from_config(&api("")).is_none());
        assert!(AuthState::from_config(&api("token_file = \"/nonexistent\"")).is_some());
        assert!(AuthState::from_config(&api("client_ca_file = \"/ca.pem\"")).is_some());
    }

    #[test]
    fn token_file_is_read_trimmed_and_reread_on_change() {
        let dir = std::env::temp_dir().join(format!("stormd-auth-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("token");
        std::fs::write(&path, "first-token\n").unwrap();
        let auth = AuthState::from_config(&api(&format!(
            "auth_token = \"inline\"\ntoken_file = {:?}",
            path.to_str().unwrap()
        )))
        .unwrap();
        assert!(auth.token_matches("first-token"));
        assert!(auth.token_matches("inline"));
        assert!(!auth.token_matches("first-token\n"));
        assert_eq!(auth.check_credentials("admin", "first-token").as_deref(), Some("admin"));

        // A different size is a change even within one mtime tick.
        std::fs::write(&path, "rotated-token-2\n").unwrap();
        assert!(auth.token_matches("rotated-token-2"));
        assert!(!auth.token_matches("first-token"));

        // Gone or empty: nothing from the file matches; the inline token still does.
        std::fs::write(&path, "  \n").unwrap();
        assert!(!auth.token_matches(""));
        std::fs::remove_file(&path).unwrap();
        assert!(!auth.token_matches("rotated-token-2"));
        assert!(auth.token_matches("inline"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
