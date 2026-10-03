use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::watch;

use crate::ui::{snapshot, AppState};

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let snap = snapshot::dashboard_snapshot(state.orch.as_ref(), state.ui.as_ref());
    if sender
        .send(Message::Text(snap.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    let mut tick_snap = tokio::time::interval(Duration::from_millis(500));
    let mut tick_meters = tokio::time::interval(Duration::from_millis(80));
    // Skip immediate first ticks after connect (already sent snapshot).
    tick_snap.tick().await;
    tick_meters.tick().await;

    let (cancel_tx, mut cancel_rx) = watch::channel(false);
    let reader = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if matches!(msg, Message::Close(_) | Message::Text(_)) {
                // Ignore client pings/text; close ends loop.
                if matches!(msg, Message::Close(_)) {
                    break;
                }
            }
        }
        let _ = cancel_tx.send(true);
    });

    loop {
        tokio::select! {
            _ = cancel_rx.changed() => {
                if *cancel_rx.borrow() { break; }
            }
            _ = tick_snap.tick() => {
                let snap = snapshot::dashboard_snapshot(state.orch.as_ref(), state.ui.as_ref());
                if sender.send(Message::Text(snap.to_string().into())).await.is_err() {
                    break;
                }
            }
            _ = tick_meters.tick() => {
                let frame = snapshot::meters_frame(state.orch.as_ref());
                if sender.send(Message::Text(frame.to_string().into())).await.is_err() {
                    break;
                }
            }
        }
    }
    reader.abort();
}

/// Keep Arc clone pattern happy for schedule task.
pub type SharedState = Arc<()>;
