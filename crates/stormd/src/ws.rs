use crate::api::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;

use serde::Deserialize;
use std::sync::Arc;

/// WebSocket handler for live process terminal output.
/// Streams raw output from the process VT100 terminal.
pub async fn ws_console(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Path(process): Path<String>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_console(socket, state, process))
}

async fn handle_console(mut socket: WebSocket, state: Arc<AppState>, process: String) {
    // Subscribe to the process log stream
    let mut rx = state.stormlog.subscribe_process(&process).await;

    // Send initial screen snapshot if available
    if let Some(snap) = state.stormlog.get_screen(&process).await {
        let msg = serde_json::json!({
            "type": "snapshot",
            "data": snap,
        });
        if socket
            .send(Message::Text(serde_json::to_string(&msg).unwrap_or_default().into()))
            .await
            .is_err()
        {
            return;
        }
    }

    // Stream live entries
    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(entry) => {
                        let msg = serde_json::json!({
                            "type": "entry",
                            "data": entry,
                        });
                        if socket
                            .send(Message::Text(serde_json::to_string(&msg).unwrap_or_default().into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        let msg = serde_json::json!({
                            "type": "lagged",
                            "skipped": n,
                        });
                        let _ = socket
                            .send(Message::Text(serde_json::to_string(&msg).unwrap_or_default().into()))
                            .await;
                    }
                    Err(_) => break,
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {} // Ignore other client messages
                }
            }
        }
    }
}

/// WebSocket handler for live component summaries. Sends the full summary
/// list on connect, then again whenever it changes — the client always holds
/// a complete, current picture and never has to merge deltas.
pub async fn ws_components(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_components(socket, state))
}

async fn handle_components(mut socket: WebSocket, state: Arc<AppState>) {
    let mut last: Option<Vec<crate::components::ComponentSummary>> = None;
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));
    loop {
        tokio::select! {
            _ = tick.tick() => {
                let now = crate::components::collect(&state).await;
                // Uptime strings advance on their own, so most ticks do
                // send; the guard only suppresses the frame when nothing at
                // all moved. A 2s cadence of small full snapshots is cheap,
                // and full snapshots spare every client a merge protocol.
                if last.as_ref() != Some(&now) {
                    let msg = serde_json::to_string(&now).unwrap_or_default();
                    if socket.send(Message::Text(msg.into())).await.is_err() {
                        break;
                    }
                    last = Some(now);
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

/// A severity by name, defaulting to Debug — which lets everything through, so
/// a filter nobody can spell does not silently blank the view.
fn severity_named(s: &str) -> stormlog::types::Severity {
    use stormlog::types::Severity as S;
    match s {
        "emerg" | "emergency" => S::Emergency,
        "alert" => S::Alert,
        "crit" | "critical" => S::Critical,
        "error" | "err" => S::Error,
        "warn" | "warning" => S::Warning,
        "notice" => S::Notice,
        "info" => S::Info,
        _ => S::Debug,
    }
}

#[derive(Debug, Deserialize)]
pub struct WsLogQuery {
    pub process: Option<String>,
    pub severity: Option<String>,
}

/// WebSocket handler for live log tailing.
/// Streams log entries matching optional filters.
pub async fn ws_logs(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(q): Query<WsLogQuery>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_logs(socket, state, q))
}

async fn handle_logs(mut socket: WebSocket, state: Arc<AppState>, query: WsLogQuery) {
    let min_severity = query.severity.as_deref().map(severity_named);

    let mut rx = match &query.process {
        Some(process) => state.stormlog.subscribe_process(process).await,
        None => state.stormlog.subscribe_all(),
    };

    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(entry) => {
                        // "At least this severe", not "exactly this". Syslog
                        // orders 0 worst, so the comparison reads backwards
                        // and is right — and a viewer asking for errors is
                        // asking to be shown a panic too, which an equality
                        // test hides at the moment it matters most.
                        if let Some(min) = min_severity {
                            if entry.severity > min {
                                continue;
                            }
                        }
                        let msg = serde_json::to_string(&entry).unwrap_or_default();
                        if socket.send(Message::Text(msg.into())).await.is_err() {
                            break;
                        }
                    }
                    // The follower fell behind and the channel dropped lines.
                    // Say so: a gap a viewer cannot see is a viewer that
                    // believes nothing happened.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        let note = serde_json::json!({
                            "process": "stormd",
                            "severity": "warn",
                            "line": format!("--- {n} line(s) dropped: this viewer fell behind ---"),
                        });
                        if socket.send(Message::Text(note.to_string().into())).await.is_err() {
                            break;
                        }
                        continue;
                    }
                    Err(_) => break,
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}
