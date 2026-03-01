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
    let mut rx = match &query.process {
        Some(process) => state.stormlog.subscribe_process(process).await,
        None => state.stormlog.subscribe_all(),
    };

    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(entry) => {
                        // Apply severity filter if specified
                        if let Some(ref sev) = query.severity {
                            let entry_sev = format!("{}", entry.severity);
                            if entry_sev != *sev {
                                continue;
                            }
                        }
                        let msg = serde_json::to_string(&entry).unwrap_or_default();
                        if socket.send(Message::Text(msg.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
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
