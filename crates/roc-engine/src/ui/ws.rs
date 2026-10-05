use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use roc_pipelines::{new_preview_signal_tx, PreviewSignal};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tracing::warn;

use crate::ui::{snapshot, AppState};

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

#[derive(Debug, Deserialize)]
struct ClientMsg {
    #[serde(rename = "type")]
    kind: String,
    channel: Option<u32>,
    pair: Option<u8>,
    sdp: Option<String>,
    candidate: Option<String>,
    #[serde(default, rename = "sdpMLineIndex")]
    sdp_mline_index: Option<u32>,
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
    let mut tick_meters = tokio::time::interval(Duration::from_millis(33));
    tick_snap.tick().await;
    tick_meters.tick().await;

    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Value>();
    let mut preview_channel: Option<u32> = None;
    let mut preview_session: Option<String> = None;
    let mut preview_rx: Option<std::sync::mpsc::Receiver<PreviewSignal>> = None;

    loop {
        // Drain GStreamer → WS preview signals without blocking the async loop.
        if let Some(rx) = preview_rx.as_ref() {
            while let Ok(sig) = rx.try_recv() {
                let _ = out_tx.send(preview_signal_json(sig));
            }
        }

        tokio::select! {
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(m) = serde_json::from_str::<ClientMsg>(&text) {
                            handle_client_msg(
                                &state,
                                &m,
                                &mut preview_channel,
                                &mut preview_session,
                                &mut preview_rx,
                                &out_tx,
                            );
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
            Some(v) = out_rx.recv() => {
                if sender.send(Message::Text(v.to_string().into())).await.is_err() {
                    break;
                }
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

    // Only tear down if this socket still owns the active session (pop-out handoff).
    if let (Some(ch), Some(sid)) = (preview_channel.take(), preview_session.take()) {
        let _ = state.orch.stop_webrtc_preview_session(ch, &sid);
    }
}

fn preview_signal_json(sig: PreviewSignal) -> Value {
    match sig {
        PreviewSignal::Offer { channel, sdp } => json!({
            "type": "preview_offer",
            "channel": channel,
            "sdp": sdp,
        }),
        PreviewSignal::Ice {
            channel,
            candidate,
            sdp_mline_index,
        } => json!({
            "type": "preview_ice",
            "channel": channel,
            "candidate": candidate,
            "sdpMLineIndex": sdp_mline_index,
        }),
        PreviewSignal::Error { channel, message } => json!({
            "type": "preview_error",
            "channel": channel,
            "message": message,
        }),
    }
}

fn handle_client_msg(
    state: &AppState,
    m: &ClientMsg,
    preview_channel: &mut Option<u32>,
    preview_session: &mut Option<String>,
    preview_rx: &mut Option<std::sync::mpsc::Receiver<PreviewSignal>>,
    out_tx: &mpsc::UnboundedSender<Value>,
) {
    match m.kind.as_str() {
        "preview_open" => {
            let Some(channel) = m.channel else { return };
            let pair = m.pair.unwrap_or(0);
            if let (Some(prev), Some(sid)) = (preview_channel.take(), preview_session.take()) {
                let _ = state.orch.stop_webrtc_preview_session(prev, &sid);
            }
            *preview_rx = None;
            let (tx, rx) = new_preview_signal_tx();
            match state.orch.start_webrtc_preview(channel, pair, tx) {
                Ok(session_id) => {
                    *preview_channel = Some(channel);
                    *preview_session = Some(session_id.clone());
                    *preview_rx = Some(rx);
                    let _ = out_tx.send(json!({
                        "type": "preview_opened",
                        "channel": channel,
                        "pair": pair,
                        "session_id": session_id,
                    }));
                }
                Err(err) => {
                    warn!(channel, error = %err, "preview_open failed");
                    let _ = out_tx.send(json!({
                        "type": "preview_error",
                        "channel": channel,
                        "message": err.to_string(),
                    }));
                }
            }
        }
        "preview_answer" => {
            let Some(channel) = m.channel.or(*preview_channel) else { return };
            let Some(sdp) = m.sdp.as_deref() else { return };
            if let Err(err) = state.orch.set_webrtc_answer(channel, sdp) {
                warn!(channel, error = %err, "preview_answer failed");
                let _ = out_tx.send(json!({
                    "type": "preview_error",
                    "channel": channel,
                    "message": err.to_string(),
                }));
            }
        }
        "preview_ice" => {
            let Some(channel) = m.channel.or(*preview_channel) else { return };
            let Some(candidate) = m.candidate.as_deref() else { return };
            let mline = m.sdp_mline_index.unwrap_or(0);
            if let Err(err) = state.orch.add_webrtc_ice(channel, mline, candidate) {
                warn!(channel, error = %err, "preview_ice failed");
            }
        }
        "preview_close" => {
            if let (Some(ch), Some(sid)) = (preview_channel.take(), preview_session.take()) {
                let _ = state.orch.stop_webrtc_preview_session(ch, &sid);
            }
            *preview_rx = None;
            let _ = out_tx.send(json!({ "type": "preview_closed" }));
        }
        _ => {}
    }
}