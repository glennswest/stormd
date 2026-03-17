use crate::backup::BackupManager;
use crate::cron::CronScheduler;
use crate::debug;
use crate::stats::StatsCollector;
use crate::supervisor::{ProcessState, Supervisor};
use crate::updater::Updater;
use crate::ws;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use stormlog::types::LogQuery as StormLogQuery;
use stormlog::StormLog;

pub struct AppState {
    pub supervisor: Arc<Supervisor>,
    pub stormlog: Arc<StormLog>,
    pub cron_scheduler: Arc<CronScheduler>,
    pub stats: Arc<StatsCollector>,
    pub backup: Arc<BackupManager>,
    pub updater: Option<Arc<Updater>>,
    pub debug_enabled: bool,
    pub allow_signal: bool,
    pub allow_stdin: bool,
    pub log_dir: std::path::PathBuf,
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
        .route("/api/v1/logs/ingest", post(ingest_log))
        .route("/api/v1/logs/stored", get(query_stored_logs))
        // Terminal
        .route("/api/v1/terminal/{process}", get(terminal_snapshot))
        // Cron
        .route("/api/v1/cron", get(list_cron_jobs))
        // Backup
        .route("/api/v1/backup", post(trigger_backup))
        // Updates
        .route("/api/v1/updates", get(list_updates))
        .route("/api/v1/updates/{name}", get(get_update))
        .route("/api/v1/updates/{name}/trigger", post(trigger_update))
        // System info
        .route("/api/v1/mounts", get(list_mounts))
        .route("/api/v1/memory/history", get(memory_history))
        // WebSocket
        .route("/ws/console/{process}", get(ws::ws_console))
        .route("/ws/logs", get(ws::ws_logs))
        // Web UI
        .route("/ui/", get(crate::web::dashboard_page))
        .route("/ui/terminal", get(crate::web::terminal_page))
        .route("/ui/logs", get(crate::web::logs_page));

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
    let lines = read_log_files(
        &state.log_dir,
        q.process.as_deref(),
        q.tail,
        q.search.as_deref(),
    )
    .await?;
    let count = lines.len();
    Ok(Json(LogResponse { lines, count }))
}

async fn process_logs(
    State(state): State<Arc<AppState>>,
    Path(process): Path<String>,
    Query(q): Query<LogTailQuery>,
) -> Result<impl IntoResponse, AppError> {
    let lines = read_log_files(
        &state.log_dir,
        Some(&process),
        q.tail,
        q.search.as_deref(),
    )
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

async fn list_log_files(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    let mut files = Vec::new();
    let mut entries = tokio::fs::read_dir(&state.log_dir).await?;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if let Ok(meta) = tokio::fs::metadata(&path).await {
            files.push(serde_json::json!({
                "name": path.file_name().unwrap_or_default().to_string_lossy(),
                "path": path.to_string_lossy(),
                "size_bytes": meta.len(),
            }));
        }
    }
    Ok(Json(files))
}

// --- Log ingest ---

async fn ingest_log(
    State(state): State<Arc<AppState>>,
    Json(req): Json<stormlog::types::IngestRequest>,
) -> Result<impl IntoResponse, AppError> {
    let entry = stormlog::types::LogEntry::new(req.process, req.stream, req.line)
        .with_severity(req.severity.unwrap_or(stormlog::types::Severity::Info));
    state.stormlog.write_entry(entry).await;
    Ok(Json(serde_json::json!({ "status": "ingested" })))
}

// --- Stored logs (MinIO) ---

#[derive(Debug, Deserialize)]
struct StoredLogQuery {
    process: Option<String>,
    stream: Option<stormlog::types::LogStream>,
    search: Option<String>,
    tail: Option<usize>,
}

async fn query_stored_logs(
    State(state): State<Arc<AppState>>,
    Query(q): Query<StoredLogQuery>,
) -> Result<impl IntoResponse, AppError> {
    let query = StormLogQuery {
        process: q.process,
        stream: q.stream,
        search: q.search,
        tail: q.tail,
        ..Default::default()
    };
    let entries = state.stormlog.query_logs(&query).await?;
    Ok(Json(entries))
}

// --- Terminal snapshot ---

async fn terminal_snapshot(
    State(state): State<Arc<AppState>>,
    Path(process): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    match state.stormlog.get_screen(&process).await {
        Some(snap) => Ok(Json(serde_json::json!(snap))),
        None => Err(AppError(anyhow::anyhow!("no terminal for process '{}'", process))),
    }
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
    state.backup.backup_logs(&state.log_dir).await?;
    Ok(Json(serde_json::json!({ "status": "backup_complete" })))
}

// --- Debug ---

async fn debug_info() -> impl IntoResponse {
    Json(debug::collect_debug_info())
}

async fn debug_config() -> impl IntoResponse {
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

// --- Updates ---

async fn list_updates(State(state): State<Arc<AppState>>) -> Result<impl IntoResponse, AppError> {
    match &state.updater {
        Some(updater) => {
            let states = updater.get_all_states().await;
            Ok(Json(serde_json::json!(states)))
        }
        None => Ok(Json(serde_json::json!({
            "error": "updater not enabled"
        }))),
    }
}

async fn get_update(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    match &state.updater {
        Some(updater) => match updater.get_state(&name).await {
            Some(s) => Ok(Json(serde_json::json!(s))),
            None => Err(AppError(anyhow::anyhow!(
                "process '{}' not tracked by updater",
                name
            ))),
        },
        None => Err(AppError(anyhow::anyhow!("updater not enabled"))),
    }
}

async fn trigger_update(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    match &state.updater {
        Some(updater) => {
            updater.trigger_update(&name).await?;
            Ok(Json(serde_json::json!({
                "status": "update_triggered",
                "process": name,
            })))
        }
        None => Err(AppError(anyhow::anyhow!("updater not enabled"))),
    }
}

// --- System info ---

async fn list_mounts() -> impl IntoResponse {
    let mounts = crate::stats::StatsCollector::get_mounts();
    Json(mounts)
}

async fn memory_history(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let history = state.stats.get_memory_history().await;
    Json(history)
}

// --- Helpers ---

async fn read_log_files(
    log_dir: &std::path::Path,
    process: Option<&str>,
    tail: Option<usize>,
    search: Option<&str>,
) -> anyhow::Result<Vec<String>> {
    let mut all_lines = Vec::new();

    let entries = tokio::fs::read_dir(log_dir).await;
    let mut entries = match entries {
        Ok(e) => e,
        Err(_) => return Ok(all_lines),
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let file_name = path.file_name().unwrap_or_default().to_string_lossy();

        if !file_name.ends_with(".log") {
            continue;
        }

        if let Some(proc_filter) = process {
            if !file_name.starts_with(proc_filter) {
                continue;
            }
        }

        if let Ok(content) = tokio::fs::read_to_string(&path).await {
            for line in content.lines() {
                if let Some(pattern) = search {
                    if !line.contains(pattern) {
                        continue;
                    }
                }
                all_lines.push(line.to_string());
            }
        }
    }

    if let Some(n) = tail {
        let start = all_lines.len().saturating_sub(n);
        all_lines = all_lines[start..].to_vec();
    }

    Ok(all_lines)
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

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        AppError(err.into())
    }
}
