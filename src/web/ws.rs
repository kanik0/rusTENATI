use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use serde::Serialize;
use tokio::sync::broadcast;

use super::AppState;

/// Event types broadcast to WebSocket clients.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum DownloadEvent {
    /// A single image was downloaded successfully.
    #[serde(rename = "download_complete")]
    DownloadComplete {
        manifest_id: String,
        canvas_id: String,
        local_path: String,
    },
    /// A download failed.
    #[serde(rename = "download_failed")]
    DownloadFailed {
        manifest_id: String,
        canvas_id: String,
        error: String,
    },
    /// Progress update for a manifest download.
    #[serde(rename = "progress")]
    Progress {
        manifest_id: String,
        completed: usize,
        total: usize,
    },
    /// A manifest download session started.
    #[serde(rename = "session_start")]
    SessionStart {
        manifest_id: String,
        total_images: usize,
    },
    /// A manifest download session ended.
    #[serde(rename = "session_end")]
    SessionEnd {
        manifest_id: String,
        downloaded: usize,
        failed: usize,
        skipped: usize,
    },
}

/// Shared event broadcaster.
#[derive(Clone)]
pub struct EventBroadcaster {
    tx: broadcast::Sender<DownloadEvent>,
}

impl EventBroadcaster {
    /// Create a new broadcaster with a buffer capacity.
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Publish an event to all connected WebSocket clients.
    pub fn publish(&self, event: DownloadEvent) {
        // Ignore errors (no subscribers connected)
        let _ = self.tx.send(event);
    }

    /// Subscribe to events.
    pub fn subscribe(&self) -> broadcast::Receiver<DownloadEvent> {
        self.tx.subscribe()
    }
}

/// WebSocket upgrade handler.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: Arc<AppState>) {
    if let Some(ref broadcaster) = state.broadcaster {
        let mut rx = broadcaster.subscribe();

        loop {
            tokio::select! {
                // Forward broadcast events to websocket
                event = rx.recv() => {
                    match event {
                        Ok(evt) => {
                            let json = match serde_json::to_string(&evt) {
                                Ok(j) => j,
                                Err(_) => continue,
                            };
                            if socket.send(Message::Text(json.into())).await.is_err() {
                                break; // Client disconnected
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::debug!("WebSocket client lagged by {n} events");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                // Handle incoming messages (ping/pong/close)
                msg = socket.recv() => {
                    match msg {
                        Some(Ok(Message::Close(_))) | None => break,
                        Some(Ok(Message::Ping(data))) => {
                            if socket.send(Message::Pong(data)).await.is_err() {
                                break;
                            }
                        }
                        _ => {} // Ignore text/binary from client
                    }
                }
            }
        }
    }
}
