//! Authentication hooks for the API and UI. Off unless `[api]` configures a
//! `password` (interactive login) or `auth_token` (machine bearer token) —
//! then everything except the health check, metrics, the auth endpoints and
//! the static UI assets requires a session cookie or a bearer token.
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
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

const SESSION_TTL: Duration = Duration::from_secs(24 * 3600);
const COOKIE: &str = "stormd_session";

pub struct AuthState {
    password: Option<String>,
    token: Option<String>,
    sessions: RwLock<HashMap<String, Instant>>,
}

impl AuthState {
    /// None when no credential is configured — auth disabled, everything open.
    pub fn from_config(api: &crate::config::ApiConfig) -> Option<Arc<Self>> {
        if api.password.is_none() && api.auth_token.is_none() {
            return None;
        }
        Some(Arc::new(Self {
            password: api.password.clone(),
            token: api.auth_token.clone(),
            sessions: RwLock::new(HashMap::new()),
        }))
    }

    fn password_matches(&self, given: &str) -> bool {
        // The bearer token doubles as a valid login password, so a machine
        // credential also opens the UI.
        [self.password.as_deref(), self.token.as_deref()]
            .into_iter()
            .flatten()
            .any(|expect| ct_eq(given, expect))
    }

    async fn new_session(&self) -> String {
        let id = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let mut sessions = self.sessions.write().await;
        sessions.retain(|_, created| created.elapsed() < SESSION_TTL);
        sessions.insert(id.clone(), Instant::now());
        id
    }

    async fn session_valid(&self, id: &str) -> bool {
        let sessions = self.sessions.read().await;
        sessions
            .get(id)
            .map(|created| created.elapsed() < SESSION_TTL)
            .unwrap_or(false)
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

/// Paths that stay open even with auth on: liveness for orchestrators,
/// metrics for scrapers, the auth endpoints themselves, and the static SPA
/// (which shows the login screen — the data behind it is what's protected).
/// The plugin proxy is NOT public: it reaches into other processes.
fn is_public(path: &str) -> bool {
    path == "/"
        || path == "/metrics"
        || path == "/api/v1/health"
        || path.starts_with("/api/v1/auth/")
        || (path.starts_with("/ui/") && !path.starts_with("/ui/proxy/"))
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

    if let Some(expect) = &auth.token {
        if let Some(given) = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
        {
            if ct_eq(given, expect) {
                return next.run(req).await;
            }
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
    password: String,
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LoginRequest>,
) -> Response {
    let Some(auth) = &state.auth else {
        return Json(serde_json::json!({ "ok": true, "required": false })).into_response();
    };
    if !auth.password_matches(&body.password) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "wrong password" })),
        )
            .into_response();
    }
    let id = auth.new_session().await;
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
        Json(serde_json::json!({ "ok": true })),
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
    let authenticated = match &state.auth {
        None => true,
        Some(auth) => match session_cookie(&req) {
            Some(id) => auth.session_valid(&id).await,
            None => false,
        },
    };
    Json(serde_json::json!({
        "required": state.auth.is_some(),
        "authenticated": authenticated,
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
        assert!(is_public("/metrics"));
        assert!(is_public("/api/v1/auth/login"));
        assert!(is_public("/ui/"));
        assert!(is_public("/ui/assets/app.js"));
        assert!(!is_public("/ui/proxy/myapp/"));
        assert!(!is_public("/api/v1/processes"));
        assert!(!is_public("/ws/logs"));
    }
}
