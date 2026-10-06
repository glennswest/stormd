use crate::backup::BackupManager;
use crate::cron::CronScheduler;
use crate::debug;
use crate::stats::StatsCollector;
use crate::supervisor::{ProcessState, Supervisor};
use crate::updater::Updater;
use crate::ws;
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{any, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use stormlog::types::LogQuery as StormLogQuery;
use stormlog::StormLog;

#[derive(Clone)]
pub struct UiPlugin {
    pub name: String,
    pub label: String,
    pub proxy_url: String,
    pub host: Option<String>,
    pub summary_url: Option<String>,
}

pub struct AppState {
    pub supervisor: Arc<Supervisor>,
    pub stormlog: Arc<StormLog>,
    pub cron_scheduler: Arc<CronScheduler>,
    pub stats: Arc<StatsCollector>,
    pub backup: Arc<BackupManager>,
    pub updater: Option<Arc<Updater>>,
    pub shutdown_tx: tokio::sync::watch::Sender<Option<i32>>,
    pub debug_enabled: bool,
    pub allow_signal: bool,
    pub allow_stdin: bool,
    pub log_dir: std::path::PathBuf,
    pub container_name: String,
    pub cloud_id: String,
    pub ui_plugins: Vec<UiPlugin>,
    /// Host: header -> redirect target path (name-based routing on the API port).
    pub host_routes: std::collections::HashMap<String, String>,
    /// None = auth off (no credential configured), everything open.
    pub auth: Option<Arc<crate::auth::AuthState>>,
    /// Configured default UI theme, served to the SPA before login.
    pub ui_theme: Option<String>,
}

pub fn build_router(state: Arc<AppState>) -> Router {
    let mut router = Router::new()
        // Name-based routing entry point
        .route("/", get(root_redirect))
        // Health & status
        .route("/api/v1/health", get(health))
        .route("/api/v1/status", get(status))
        .route("/api/v1/stats", get(stats))
        // Prometheus text format, at the path everything that scrapes expects.
        // Not under /api/v1: a scraper is configured with a port and a path,
        // and every one of them defaults to this one.
        .route("/metrics", get(metrics))
        .route("/api/v1/cloudid", get(get_cloud_id))
        // Component summaries — the one feed both dashboards render from
        .route("/api/v1/components", get(components))
        // Auth — open endpoints; the middleware guards everything else
        .route("/api/v1/auth/login", post(crate::auth::login))
        .route("/api/v1/auth/logout", post(crate::auth::logout))
        .route("/api/v1/auth/session", get(crate::auth::session))
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
        .route("/api/v1/logs/{process}/runs", get(list_runs))
        .route("/api/v1/logs/files/{filename}", get(read_log_file))
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
        .route("/ws/components", get(ws::ws_components))
        // Shutdown
        .route("/api/v1/shutdown", post(shutdown))
        // Web UI — embedded SPA (legacy page URLs redirect inside the handler)
        .route("/ui/", get(crate::web::index))
        // Plugin UI
        .route("/ui/proxy/{name}", any(proxy_plugin))
        .route("/ui/proxy/{name}/", any(proxy_plugin))
        .route("/ui/proxy/{name}/{*rest}", any(proxy_plugin))
        // Static segments outrank the catch-all, so /ui/proxy stays proxied
        .route("/ui/{*path}", get(crate::web::asset))
        .route("/api/v1/plugins", get(list_plugins));

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

    router
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require_auth,
        ))
        .with_state(state)
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

async fn get_cloud_id(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(serde_json::json!({
        "cloud_id": state.cloud_id,
        "container_name": state.container_name,
    }))
}

async fn components(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(crate::components::collect(&state).await)
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

/// Prometheus text format, for whatever is scraping.
///
/// Metrics are not events and are not logs. Kubernetes keeps none of this in
/// its datastore — `kubectl top` is served by metrics-server out of memory and
/// persists nothing — because a number sampled every fifteen seconds forever is
/// the one kind of data a consensus store must never be asked to hold. So this
/// is a reading taken on request, kept nowhere, and true at the moment it is
/// asked for.
///
/// The names follow the conventions a Prometheus consumer expects: a `_total`
/// suffix on counters, `_seconds` and `_bytes` on units, and one label set per
/// series. `container` is this stormd's name, `process` the supervised binary —
/// so a fleet's worth of these aggregate by either without anything having to
/// rewrite them.
async fn metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    use std::fmt::Write;

    let statuses = state.supervisor.get_all_statuses().await;
    let sys = state.stats.get_stats().await;
    let c = &sys.container_name;
    let mut o = String::with_capacity(2048);

    let _ = writeln!(o, "# HELP stormd_up 1 when this supervisor is answering.");
    let _ = writeln!(o, "# TYPE stormd_up gauge");
    let _ = writeln!(o, "stormd_up{{container=\"{c}\"}} 1");

    // The supervisor's own process, under the names every Prometheus client
    // library uses for the exporting process. Not stormd_-prefixed inventions:
    // a dashboard or an alert written against a Kubernetes component works
    // against this one unchanged, and that is the whole value of a convention.
    let _ = writeln!(o, "# HELP process_start_time_seconds Start time since the epoch.");
    let _ = writeln!(o, "# TYPE process_start_time_seconds gauge");
    let _ = writeln!(
        o,
        "process_start_time_seconds{{container=\"{c}\"}} {}",
        sys.started_at.timestamp()
    );

    if let Some(m) = &sys.memory {
        let _ = writeln!(o, "# HELP process_resident_memory_bytes Resident memory.");
        let _ = writeln!(o, "# TYPE process_resident_memory_bytes gauge");
        let _ = writeln!(
            o,
            "process_resident_memory_bytes{{container=\"{c}\"}} {}",
            m.rss_bytes
        );
        let _ = writeln!(o, "# HELP process_virtual_memory_bytes Virtual memory.");
        let _ = writeln!(o, "# TYPE process_virtual_memory_bytes gauge");
        let _ = writeln!(
            o,
            "process_virtual_memory_bytes{{container=\"{c}\"}} {}",
            m.vms_bytes
        );
    }

    let _ = writeln!(o, "# HELP stormd_uptime_seconds How long this supervisor has been up.");
    let _ = writeln!(o, "# TYPE stormd_uptime_seconds gauge");
    let _ = writeln!(o, "stormd_uptime_seconds{{container=\"{c}\"}} {}", sys.uptime_secs);

    let _ = writeln!(
        o,
        "# HELP stormd_process_state 1 for the state this process is in, 0 otherwise."
    );
    let _ = writeln!(o, "# TYPE stormd_process_state gauge");
    for p in &statuses {
        for st in ["running", "stopped", "failed", "starting", "restarting"] {
            let now = format!("{:?}", p.state).to_lowercase();
            let v = if now == st { 1 } else { 0 };
            let _ = writeln!(
                o,
                "stormd_process_state{{container=\"{c}\",process=\"{}\",state=\"{st}\"}} {v}",
                p.name
            );
        }
    }

    // The counter Kubernetes keeps on the Pod as
    // status.containerStatuses[].restartCount, at the level this supervisor
    // owns: a process inside a container rather than a container inside a pod.
    let _ = writeln!(o, "# HELP stormd_process_restarts_total Restarts since this supervisor started.");
    let _ = writeln!(o, "# TYPE stormd_process_restarts_total counter");
    for p in &statuses {
        let _ = writeln!(
            o,
            "stormd_process_restarts_total{{container=\"{c}\",process=\"{}\"}} {}",
            p.name, p.restarts
        );
    }

    let _ = writeln!(o, "# HELP stormd_process_crashes_total Non-zero exits since this supervisor started.");
    let _ = writeln!(o, "# TYPE stormd_process_crashes_total counter");
    for p in &statuses {
        let _ = writeln!(
            o,
            "stormd_process_crashes_total{{container=\"{c}\",process=\"{}\"}} {}",
            p.name, p.crashes
        );
    }

    let _ = writeln!(o, "# HELP stormd_process_liveness_failures_total Liveness probe failures.");
    let _ = writeln!(o, "# TYPE stormd_process_liveness_failures_total counter");
    for p in &statuses {
        let _ = writeln!(
            o,
            "stormd_process_liveness_failures_total{{container=\"{c}\",process=\"{}\"}} {}",
            p.name, p.liveness_failures
        );
    }

    let _ = writeln!(o, "# HELP stormd_process_uptime_seconds How long this process has been up.");
    let _ = writeln!(o, "# TYPE stormd_process_uptime_seconds gauge");
    for p in &statuses {
        // Absent rather than zero when it is not running: zero is a running
        // process that has just started, and the two must not read alike.
        if let Some(u) = p.uptime_secs {
            let _ = writeln!(
                o,
                "stormd_process_uptime_seconds{{container=\"{c}\",process=\"{}\"}} {u}",
                p.name
            );
        }
    }

    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        o,
    )
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

/// Read a specific log file by name (with optional tail).
async fn read_log_file(
    State(state): State<Arc<AppState>>,
    Path(filename): Path<String>,
    Query(q): Query<LogTailQuery>,
) -> Result<impl IntoResponse, AppError> {
    // Sanitize filename — no path traversal
    if filename.contains('/') || filename.contains("..") {
        return Err(AppError(anyhow::anyhow!("invalid filename")));
    }
    let path = state.log_dir.join(&filename);
    if !path.exists() {
        return Err(AppError(anyhow::anyhow!("file not found: {}", filename)));
    }
    let content = tokio::fs::read_to_string(&path).await?;
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();

    if let Some(pattern) = q.search.as_deref() {
        lines.retain(|l| l.contains(pattern));
    }

    if let Some(n) = q.tail {
        let start = lines.len().saturating_sub(n);
        lines = lines[start..].to_vec();
    }

    let count = lines.len();
    Ok(Json(LogResponse { lines, count }))
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

// --- Stored logs (what is on the log volume) ---

#[derive(Debug, Deserialize)]
struct StoredLogQuery {
    process: Option<String>,
    stream: Option<stormlog::types::LogStream>,
    search: Option<String>,
    tail: Option<usize>,
    run_id: Option<String>,
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
        run_id: q.run_id,
        ..Default::default()
    };
    let entries = state.stormlog.query_logs(&query).await?;
    Ok(Json(entries))
}

/// List all runs for a process (newest first).
async fn list_runs(
    State(state): State<Arc<AppState>>,
    Path(process): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let runs = state.stormlog.list_runs(&process).await?;
    let current = state.stormlog.current_run_id(&process).await;
    Ok(Json(serde_json::json!({
        "process": process,
        "current_run_id": current,
        "runs": runs,
    })))
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

// --- Shutdown ---

#[derive(Deserialize, Default)]
struct ShutdownRequest {
    #[serde(default, rename = "exitCode")]
    exit_code: Option<i32>,
}

async fn shutdown(
    State(state): State<Arc<AppState>>,
    body: Option<Json<ShutdownRequest>>,
) -> impl IntoResponse {
    let code = body.and_then(|b| b.0.exit_code).unwrap_or(0);
    let _ = state.shutdown_tx.send(Some(code));
    Json(serde_json::json!({ "status": "shutting down" }))
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

// --- Plugins ---

async fn list_plugins(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let plugins: Vec<_> = state
        .ui_plugins
        .iter()
        .map(|p| {
            serde_json::json!({
                "name": p.name,
                "label": p.label,
                "path": format!("/ui/ext/{}", p.name),
            })
        })
        .collect();
    Json(serde_json::Value::Array(plugins))
}

/// Root handler: route by `Host:` header. Looks up the (config-driven) host
/// map and redirects to its target; unknown hosts fall back to the dashboard.
async fn root_redirect(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> axum::response::Redirect {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(|h| h.split(':').next().unwrap_or(h).to_ascii_lowercase())
        .unwrap_or_default();
    let target = state
        .host_routes
        .get(&host)
        .cloned()
        .unwrap_or_else(|| "/ui/".to_string());
    axum::response::Redirect::temporary(&target)
}

async fn proxy_plugin(
    State(state): State<Arc<AppState>>,
    method: axum::http::Method,
    OriginalUri(uri): OriginalUri,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<axum::response::Response, AppError> {
    let path = uri.path();
    let after = path.strip_prefix("/ui/proxy/").unwrap_or("");
    let (name, subpath) = match after.find('/') {
        Some(i) => (&after[..i], &after[i + 1..]),
        None => (after, ""),
    };

    let plugin = state
        .ui_plugins
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| AppError(anyhow::anyhow!("plugin '{}' not found", name)))?;

    let query = uri
        .query()
        .map(|q| format!("?{}", q))
        .unwrap_or_default();
    let target = format!(
        "{}/{}{}",
        plugin.proxy_url.trim_end_matches('/'),
        subpath,
        query
    );

    let own_token = state.auth.as_ref().and_then(|a| a.token());
    proxy_to(&target, method, &headers, body, own_token).await
}

/// Hop-by-hop headers (RFC 9110 §7.6.1) plus the ones the client library
/// sets itself for the new connection. Never forwarded either way.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
];

fn is_hop_by_hop(name: &axum::http::HeaderName, connection: &[String]) -> bool {
    let n = name.as_str();
    HOP_BY_HOP.contains(&n) || connection.iter().any(|c| c == n)
}

/// Header names listed in `Connection:` are hop-by-hop for this message too.
fn connection_tokens(headers: &axum::http::HeaderMap) -> Vec<String> {
    headers
        .get_all(axum::http::header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

/// What goes to the plugin: the client's headers minus hop-by-hop ones and
/// minus stormd's own credentials. A plugin's `Authorization` (its own
/// bearer, e.g. stormstorage's `api_token`) passes through; one carrying
/// stormd's `auth_token` was for stormd and stops here, as does the
/// `stormd_session` cookie.
fn upstream_request_headers(
    headers: &axum::http::HeaderMap,
    own_token: Option<&str>,
) -> axum::http::HeaderMap {
    use axum::http::header::{AUTHORIZATION, COOKIE};
    let connection = connection_tokens(headers);
    let mut out = axum::http::HeaderMap::new();
    for (name, value) in headers {
        if is_hop_by_hop(name, &connection) {
            continue;
        }
        if name == AUTHORIZATION {
            let bearer = value.to_str().ok().and_then(|v| v.strip_prefix("Bearer "));
            if matches!((bearer, own_token), (Some(b), Some(t)) if b == t) {
                continue;
            }
        }
        if name == COOKIE {
            let kept: Vec<&str> = value
                .to_str()
                .unwrap_or("")
                .split(';')
                .map(str::trim)
                .filter(|c| !c.is_empty() && !crate::auth::is_session_cookie(c))
                .collect();
            if !kept.is_empty() {
                if let Ok(v) = axum::http::HeaderValue::from_str(&kept.join("; ")) {
                    out.append(COOKIE, v);
                }
            }
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out
}

/// What comes back from the plugin: its headers minus hop-by-hop ones, so
/// `Set-Cookie`, `Location`, caching and encoding headers survive. A
/// `Set-Cookie` for stormd's own session cookie is dropped — a plugin
/// must not be able to sign the browser in or out of stormd.
fn downstream_response_headers(headers: &axum::http::HeaderMap) -> axum::http::HeaderMap {
    use axum::http::header::SET_COOKIE;
    let connection = connection_tokens(headers);
    let mut out = axum::http::HeaderMap::new();
    for (name, value) in headers {
        if is_hop_by_hop(name, &connection) {
            continue;
        }
        if name == SET_COOKIE
            && value
                .to_str()
                .map(crate::auth::is_session_cookie)
                .unwrap_or(false)
        {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out
}

/// One client for every proxied request (connection reuse). Redirects are
/// not followed: a plugin's `Location` goes back to the browser.
fn proxy_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("proxy client")
    })
}

async fn proxy_to(
    target: &str,
    method: axum::http::Method,
    headers: &axum::http::HeaderMap,
    body: axum::body::Bytes,
    own_token: Option<&str>,
) -> Result<axum::response::Response, AppError> {
    let mut builder = proxy_client()
        .request(method, target)
        .headers(upstream_request_headers(headers, own_token));
    if !body.is_empty() {
        builder = builder.body(body);
    }
    let resp = builder
        .send()
        .await
        .map_err(|e| AppError(anyhow::anyhow!("proxy: {}", e)))?;

    let status = resp.status();
    let resp_headers = downstream_response_headers(resp.headers());
    let resp_body = resp
        .bytes()
        .await
        .map_err(|e| AppError(anyhow::anyhow!("proxy: {}", e)))?;

    let mut response = axum::http::Response::new(axum::body::Body::from(resp_body));
    *response.status_mut() = status;
    *response.headers_mut() = resp_headers;
    Ok(response)
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

#[cfg(test)]
mod proxy_tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue, Method};
    use std::sync::Mutex;

    #[derive(Default)]
    struct Seen {
        method: String,
        path: String,
        headers: HeaderMap,
        body: Vec<u8>,
    }

    /// A stand-in plugin: records what reaches it, answers with a cookie,
    /// a redirect target and a hop-by-hop header that must not come back.
    async fn upstream() -> (String, Arc<Mutex<Seen>>) {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let st = seen.clone();
        let app = Router::new().fallback(
            move |method: Method, uri: axum::http::Uri, headers: HeaderMap, body: axum::body::Bytes| {
                let st = st.clone();
                async move {
                    *st.lock().unwrap() = Seen {
                        method: method.to_string(),
                        path: uri.to_string(),
                        headers,
                        body: body.to_vec(),
                    };
                    axum::http::Response::builder()
                        .status(302)
                        .header("set-cookie", "plugin_sid=abc; Path=/")
                        .header("set-cookie", "stormd_session=evil; Path=/")
                        .header("location", "/login")
                        .header("x-plugin", "yes")
                        .header("keep-alive", "timeout=5")
                        .body(axum::body::Body::from("moved"))
                        .unwrap()
                }
            },
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (url, seen)
    }

    fn h(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.append(*k, HeaderValue::from_str(v).unwrap());
        }
        m
    }

    #[tokio::test]
    async fn plugin_bearer_reaches_the_plugin_and_cookies_come_back() {
        let (url, seen) = upstream().await;
        let req = h(&[
            ("authorization", "Bearer plugin-token"),
            ("content-type", "application/json"),
            ("cookie", "stormd_session=secret; plugin_sid=old"),
            ("x-requested-with", "fetch"),
            ("connection", "keep-alive, x-drop-me"),
            ("x-drop-me", "1"),
            ("host", "stormd.example"),
        ]);
        let body = axum::body::Bytes::from_static(b"{\"size\":\"1G\"}\xff");
        let resp = proxy_to(
            &format!("{}/api/volumes?x=1", url),
            Method::POST,
            &req,
            body.clone(),
            Some("stormd-token"),
        )
        .await
        .ok()
        .unwrap();

        let s = seen.lock().unwrap();
        assert_eq!(s.method, "POST");
        assert_eq!(s.path, "/api/volumes?x=1");
        assert_eq!(s.headers["authorization"], "Bearer plugin-token");
        assert_eq!(s.headers["content-type"], "application/json");
        assert_eq!(s.headers["x-requested-with"], "fetch");
        assert_eq!(s.headers["cookie"], "plugin_sid=old");
        assert!(s.headers.get("x-drop-me").is_none());
        assert_ne!(s.headers["host"], "stormd.example");
        assert_eq!(s.body, body.to_vec(), "body is passed as bytes, not text");

        assert_eq!(resp.status(), 302, "redirects are returned, not followed");
        let rh = resp.headers();
        assert_eq!(rh["location"], "/login");
        assert_eq!(rh["x-plugin"], "yes");
        let cookies: Vec<_> = rh.get_all("set-cookie").iter().collect();
        assert_eq!(cookies, vec!["plugin_sid=abc; Path=/"]);
        assert!(rh.get("keep-alive").is_none());
    }

    #[test]
    fn stormds_own_bearer_stops_at_the_proxy() {
        let req = h(&[("authorization", "Bearer stormd-token"), ("cookie", "stormd_session=s")]);
        let out = upstream_request_headers(&req, Some("stormd-token"));
        assert!(out.get("authorization").is_none());
        assert!(out.get("cookie").is_none());
        // With stormd auth off there is no token of its own to strip.
        let out = upstream_request_headers(&req, None);
        assert_eq!(out["authorization"], "Bearer stormd-token");
    }

    #[tokio::test]
    async fn get_without_body_and_other_methods() {
        let (url, seen) = upstream().await;
        for m in [Method::GET, Method::OPTIONS, Method::DELETE] {
            proxy_to(&format!("{}/", url), m.clone(), &HeaderMap::new(), Default::default(), None)
                .await
                .ok()
                .unwrap();
            let s = seen.lock().unwrap();
            assert_eq!(s.method, m.as_str());
            assert!(s.body.is_empty());
        }
    }
}
