//! Workflow metadata + thin adapter manifest for tools/clients.

use serde_json::{json, Value};

/// Machine-readable endpoint map for the engine (UI is the primary client).
///
/// Cutover from `roc-recording` (Go) is complete — this is not a parallel-proxy
/// contract anymore. Kept for health/tools that still GET `/api/adapter/manifest`.
pub fn adapter_manifest() -> Value {
    json!({
        "name": "roc-media-engine",
        "version": "0.1.0",
        "migration": {
            "mode": "complete",
            "legacy_project": "roc-recording",
            "note": "Go backend is frozen reference only; do not run roc-recording.service alongside the engine"
        },
        "endpoints": {
            "health": "GET /api/health",
            "devices": "GET /api/devices",
            "channels": "GET /api/channels",
            "capture_start": "POST /api/channels/{id}/start",
            "capture_stop": "POST /api/channels/{id}/stop",
            "record_start": "POST /api/channels/{id}/record/start?label=",
            "record_stop": "POST /api/channels/{id}/record/stop",
            "record_proxy_start": "POST /api/channels/{id}/record/proxy/start",
            "record_proxy_stop": "POST /api/channels/{id}/record/proxy/stop",
            "record_hq_start": "POST /api/channels/{id}/record/hq/start",
            "record_hq_stop": "POST /api/channels/{id}/record/hq/stop",
            "srt_start": "POST /api/channels/{id}/srt/start",
            "srt_stop": "POST /api/channels/{id}/srt/stop",
            "encode_preset": "POST /api/channels/{id}/encode-preset  body: {\"preset\":\"hq\"}",
            "record_preset": "POST /api/channels/{id}/record-preset  body: {\"preset\":\"dnxhd_185\"}",
            "ui_encode_preset": "PUT /api/streams/{id}/encode-preset  body: {\"preset\":\"hq\"}",
            "ui_record_preset": "PUT /api/streams/{id}/record-preset  body: {\"preset\":\"dnxhd_185\"}",
            "ui_workflow_set": "PUT /api/workflows/{id}  body: {\"mode\":\"pair|tc\"}",
            "meters": "GET /api/meters",
            "playout_list": "GET /api/playout",
            "playout_start": "POST /api/playout/{id}/start  body: {\"source\":\"srt://...\"}",
            "playout_stop": "POST /api/playout/{id}/stop",
            "workflows": "GET /api/workflows"
        },
        "workflows": {
            "pair": "default encode + playout pair",
            "tc": "DeckLink IN → timecode overlay → DeckLink OUT (+ HLS preview)",
            "remote_commentator": "not implemented — UI option removed until WebRTC bridge lands"
        }
    })
}
