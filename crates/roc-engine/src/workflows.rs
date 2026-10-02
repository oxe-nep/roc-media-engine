//! Fas 4–5: workflow stubs + UI/Go adapter contract.

use serde_json::{json, Value};

/// Manifest describing how `roc-recording` (Go) or Next.js can talk to this engine.
pub fn adapter_manifest() -> Value {
    json!({
        "name": "roc-media-engine",
        "version": "0.1.0",
        "migration": {
            "mode": "parallel_greenfield",
            "legacy_project": "roc-recording",
            "feature_flag": "media_engine",
            "recommended": "Go adapter proxies per-channel encode/REC/SRT to this API while other channels stay on FFmpeg"
        },
        "endpoints": {
            "health": "GET /api/health",
            "devices": "GET /api/devices",
            "channels": "GET /api/channels",
            "capture_start": "POST /api/channels/{id}/start",
            "capture_stop": "POST /api/channels/{id}/stop",
            "record_start": "POST /api/channels/{id}/record/start?label=",
            "record_stop": "POST /api/channels/{id}/record/stop",
            "srt_start": "POST /api/channels/{id}/srt/start",
            "srt_stop": "POST /api/channels/{id}/srt/stop",
            "encode_preset": "POST /api/channels/{id}/encode-preset  body: {\"preset\":\"hq\"}",
            "encode_presets_list": "GET /api/encode/presets",
            "encode_presets_create": "POST /api/encode/presets  body: {id,label,video_codec,video_bitrate,...}",
            "encode_presets_upsert": "PUT /api/encode/presets/{id}",
            "encode_presets_delete": "DELETE /api/encode/presets/{id}",
            "meters": "GET /api/meters",
            "playout_list": "GET /api/playout",
            "playout_start": "POST /api/playout/{id}/start  body: {\"source\":\"srt://...\"}",
            "playout_stop": "POST /api/playout/{id}/stop",
            "workflows": "GET /api/workflows",
            "workflow_set": "POST /api/workflows/{channel_id}  body: {\"kind\":\"tc|commentator|pair\",\"active\":true}"
        },
        "workflows": {
            "pair": "default encode+decode pair (current product mode)",
            "tc": "stub — cairooverlay / clockoverlay burn-in (no FFmpeg drawtext file reload)",
            "commentator": "stub — appsrc/appsink WebRTC bridge (no pipe+FFmpeg)"
        },
        "go_adapter_sketch": {
            "env": "MEDIA_ENGINE_URL=http://127.0.0.1:8090",
            "per_channel": "if runtime flag media_engine: HTTP to engine; else legacy FFmpeg managers",
            "status_merge": "dashboard WS merges engine channel snapshots with legacy ones"
        }
    })
}
