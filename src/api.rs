use crate::backup::BackupManager;
use crate::cron::CronScheduler;
use crate::debug;
use crate::logger::LogManager;
use crate::stats::StatsCollector;
use crate::supervisor::{ProcessState, Supervisor};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub struct AppState {
    pub supervisor: Arc<Supervisor>,
    pub log_manager: Arc<LogManager>,
    pub cron_scheduler: Arc<CronScheduler>,
    pub stats: Arc<StatsCollector>,
    pub backup: Arc<BackupManager>,
    pub debug_enabled: bool,
    pub allow_signal: bool,
    pub allow_stdin: bool,
}

pub fn build_router(state: Arc<AppState>) -> Router {
    let mut router = Router::new()
        // Health & status
        .route("/api/v1/health", get(health))
        .route("/api/v1/status", get(status))
        .route("/api/v1/stats", get(stats))
        // Processes
        .route("/api/v1/processes", get(list_processes))
        .route("/api/v1/processes/{name}", get(get_process))
        .route("/api/v1/processes/{name}/start", post(start_process))
        .route("/api/v1/processes/{name}/stop", post(stop_process))
        .route("/api/v1/processes/{name}/restart", post(restart_process))
        // Logs
        .route("/api/v1/logs", get(query_logs))
        .route("/api/v1/logs/files", get(list_log_files))
        .route("/api/v1/logs/{process}", get(process_logs))
        // Cron
        .route("/api/v1/cron", get(list_cron_jobs))
        // Backup
        .route("/api/v1/backup", post(trigger_backup));

    // Debug endpoints (only if enabled)
    if state.debug_enabled {
        router = router
            .route("/api/v1/debug/info", get(debug_info))
            .route("/api/v1/debug/config", get(debug_config));

        if state.allow_signal {
            router = router.route(
                "/api/v1/debug/processes/{name}/signal",
                post(send_signal),
            );
        }

        if state.allow_stdin {
            router = router.route(
                "/api/v1/debug/processes/{name}/stdin",
                post(send_stdin),
            );
        }
    }

    router.with_state(state)
}

// --- Health & Status ---

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let processes = state.supervisor.get_all_statuses().await;
    let failed = state.supervisor.has_failed().await;
    let cron_jobs = state.cron_scheduler.get_status().await;
    let stats = state.stats.get_stats().await;

    Json(serde_json::json!({
        "container_failed": failed,
        "stats": stats,
        "processes": processes,
        "cron_jobs": cron_jobs,
    }))
}

async fn stats(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Refresh process stats
    let statuses = state.supervisor.get_all_statuses().await;
    let total = statuses.len();
    let running = statuses.iter().filter(|s| s.state == ProcessState::Running).count();
    let failed = statuses.iter().filter(|s| s.state == ProcessState::Failed).count();
    let restarts: u32 = statuses.iter().map(|s| s.restarts).sum();
    state.stats.update_process_stats(total, running, failed, restarts).await;

    let sys_stats = state.stats.get_stats().await;
    Json(sys_stats)
}

// --- Processes ---

async fn list_processes(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let statuses = state.supervisor.get_all_statuses().await;
    Json(statuses)
}

async fn get_process(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let status = state.supervisor.get_status(&name).await?;
    Ok(Json(status))
}

async fn start_process(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    state.supervisor.start_process(&name).await?;
    Ok(Json(serde_json::json!({ "status": "started", "process": name })))
}

async fn stop_process(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    state.supervisor.stop_process(&name).await?;
    Ok(Json(serde_json::json!({ "status": "stopped", "process": name })))
}

async fn restart_process(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    state.supervisor.restart_process(&name).await?;
    Ok(Json(serde_json::json!({ "status": "restarted", "process": name })))
}

// --- Logs ---

#[derive(Debug, Deserialize)]
struct LogQuery {
    process: Option<String>,
    tail: Option<usize>,
    search: Option<String>,
}

async fn query_logs(
    State(state): State<Arc<AppState>>,
    Query(q): Query<LogQuery>,
) -> Result<impl IntoResponse, AppError> {
    let lines = state
        .log_manager
        .read_logs(q.process.as_deref(), q.tail, q.search.as_deref())
        .await?;
    let count = lines.len();
    Ok(Json(LogResponse { lines, count }))
}

async fn process_logs(
    State(state): State<Arc<AppState>>,
    Path(process): Path<String>,
    Query(q): Query<LogTailQuery>,
) -> Result<impl IntoResponse, AppError> {
    let lines = state
        .log_manager
        .read_logs(Some(&process), q.tail, q.search.as_deref())
        .await?;
    let count = lines.len();
    Ok(Json(LogResponse { lines, count }))
}

#[derive(Debug, Deserialize)]
struct LogTailQuery {
    tail: Option<usize>,
    search: Option<String>,
}

#[derive(Serialize)]
struct LogResponse {
    count: usize,
    lines: Vec<String>,
}

async fn list_log_files(State(state): State<Arc<AppState>>) -> Result<impl IntoResponse, AppError> {
    let files = state.log_manager.list_files().await?;
    Ok(Json(files))
}

// --- Cron ---

async fn list_cron_jobs(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let jobs = state.cron_scheduler.get_status().await;
    Json(jobs)
}

// --- Backup ---

async fn trigger_backup(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    state.backup.backup_logs(state.log_manager.log_dir()).await?;
    Ok(Json(serde_json::json!({ "status": "backup_complete" })))
}

// --- Debug ---

async fn debug_info() -> impl IntoResponse {
    Json(debug::collect_debug_info())
}

async fn debug_config() -> impl IntoResponse {
    // Return env vars (config is loaded from file, env vars show runtime state)
    let env: Vec<(String, String)> = std::env::vars().collect();
    Json(serde_json::json!({ "environment": env }))
}

#[derive(Deserialize)]
struct SignalBody {
    signal: String,
}

async fn send_signal(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(body): Json<SignalBody>,
) -> Result<impl IntoResponse, AppError> {
    let status = state.supervisor.get_status(&name).await?;
    let pid = status.pid.ok_or_else(|| anyhow::anyhow!("process has no pid"))?;
    debug::send_signal(pid, &body.signal)?;
    Ok(Json(serde_json::json!({
        "status": "signal_sent",
        "process": name,
        "signal": body.signal,
        "pid": pid,
    })))
}

#[derive(Deserialize)]
struct StdinBody {
    input: String,
}

async fn send_stdin(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(body): Json<StdinBody>,
) -> Result<impl IntoResponse, AppError> {
    state.supervisor.send_stdin(&name, &body.input).await?;
    Ok(Json(serde_json::json!({ "status": "sent", "process": name })))
}

// --- Error handling ---

struct AppError(anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let body = serde_json::json!({
            "error": self.0.to_string(),
        });
        (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
    }
}

impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        AppError(err)
    }
}
