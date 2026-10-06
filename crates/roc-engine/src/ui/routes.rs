use std::path::PathBuf;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::fs::File;
use roc_pipelines::RecordingRole;
use tokio_util::io::ReaderStream;

use crate::ui::library;
use crate::ui::snapshot;
use crate::ui::tc;
use crate::ui::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/streams", get(list_streams))
        .route("/api/streams/{id}/start", post(start_stream))
        .route("/api/streams/{id}/stop", post(stop_stream))
        .route("/api/streams/{id}/logs", get(stream_logs))
        .route("/api/streams/{id}/encode-preset", put(set_stream_preset))
        .route("/api/streams/{id}/record-preset", put(set_stream_record_preset))
        .route("/api/recordings", get(list_recordings))
        // `/start` + `/stop` stay as HQ aliases for older UI builds.
        .route("/api/recordings/{id}/start", post(start_recording))
        .route("/api/recordings/{id}/stop", post(stop_recording))
        .route("/api/recordings/{id}/proxy/start", post(start_proxy_recording))
        .route("/api/recordings/{id}/proxy/stop", post(stop_proxy_recording))
        .route("/api/recordings/{id}/hq/start", post(start_hq_recording))
        .route("/api/recordings/{id}/hq/stop", post(stop_hq_recording))
        .route("/api/recordings/{id}/name", put(set_rec_name))
        .route("/api/recordings/{id}/category", put(set_rec_category))
        .route("/api/recordings/{id}/schedule", put(set_schedule).delete(clear_schedule))
        .route("/api/srt", get(list_srt))
        .route("/api/srt/{id}", get(get_srt).put(put_srt))
        .route("/api/srt/{id}/start", post(start_srt))
        .route("/api/srt/{id}/stop", post(stop_srt))
        .route("/api/encode/presets", get(list_presets_ui).post(create_preset_ui))
        .route(
            "/api/encode/presets/{id}",
            put(upsert_preset_ui).delete(delete_preset_ui),
        )
        .route("/api/encode/options", get(encode_options))
        .route("/api/playout", get(list_playout_ui))
        .route("/api/playout/media", get(list_playout_media).post(upload_playout_media))
        .route("/api/playout/media/{id}", axum::routing::delete(delete_playout_media))
        .route(
            "/api/playout/{id}",
            get(get_playout_ui).put(put_playout_ui),
        )
        .route("/api/playout/{id}/start", post(start_playout_ui))
        .route("/api/playout/{id}/stop", post(stop_playout_ui))
        .route("/api/playout/{id}/pause", post(pause_playout_ui))
        .route("/api/playout/{id}/resume", post(resume_playout_ui))
        .route("/api/playout/{id}/seek", post(seek_playout_ui))
        .route("/api/playout/{id}/logs", get(playout_logs_ui))
        .route("/api/playout/{id}/tc-loop", get(get_tc_loop).put(put_tc_loop))
        .route("/api/playout/devices", get(playout_devices))
        .route("/api/library/categories", get(lib_categories).post(lib_create_cat))
        .route(
            "/api/library/categories/{name}",
            put(lib_rename_cat).delete(lib_delete_cat),
        )
        .route("/api/library/files", get(lib_files))
        .route(
            "/api/library/file/{category}/{name}",
            get(lib_file_get).delete(lib_file_delete),
        )
        .route("/api/library/move", post(lib_move))
        .route(
            "/api/settings/recordings-path",
            get(get_rec_path).put(set_rec_path),
        )
        .route("/api/system", get(system_status))
        .route("/api/workflows", get(workflows_map))
        .route("/api/workflows/{id}", put(set_workflow_ui))
        .route("/thumb/{id}", get(thumb))
        .route("/thumb/playout/{id}", get(thumb_playout))
}

async fn list_streams(State(st): State<AppState>) -> Json<Value> {
    let mut out = Vec::new();
    for ch in st.orch.list_channels() {
        st.ui.ensure_channel(ch.id, &ch.name);
        if let Some(s) = snapshot::stream_json(st.orch.as_ref(), st.ui.as_ref(), ch.id) {
            out.push(s);
        }
    }
    Json(Value::Array(out))
}

async fn start_stream(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<Value>, UiError> {
    if st.ui.workflow_mode(id) == "tc" || st.ui.tc(id).enabled {
        return Err(UiError::bad("stop TC burn-in before starting encode"));
    }
    st.orch.start_capture(id).map_err(UiError::from)?;
    st.ui.set_encode_wanted(id, true);
    snapshot::stream_json(st.orch.as_ref(), st.ui.as_ref(), id)
        .map(Json)
        .ok_or_else(|| UiError::not_found("channel not found"))
}

async fn stop_stream(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<Value>, UiError> {
    let _ = st.orch.stop_srt(id);
    let _ = st.orch.stop_proxy_recording(id);
    let _ = st.orch.stop_hq_recording(id);
    st.ui.mark_recording_stopped_role(id, RecordingRole::Proxy);
    st.ui.mark_recording_stopped_role(id, RecordingRole::Hq);
    st.orch.stop_capture(id).map_err(UiError::from)?;
    st.ui.set_encode_wanted(id, false);
    snapshot::stream_json(st.orch.as_ref(), st.ui.as_ref(), id)
        .map(Json)
        .ok_or_else(|| UiError::not_found("channel not found"))
}

async fn stream_logs(Path(_id): Path<u32>) -> Json<Value> {
    Json(json!({ "lines": [] }))
}

#[derive(Deserialize)]
struct PresetBody {
    preset: String,
}

async fn set_stream_preset(
    State(st): State<AppState>,
    Path(id): Path<u32>,
    Json(body): Json<PresetBody>,
) -> Result<Json<Value>, UiError> {
    st.orch
        .set_encode_preset(id, body.preset.trim())
        .map_err(UiError::from)?;
    snapshot::stream_json(st.orch.as_ref(), st.ui.as_ref(), id)
        .map(Json)
        .ok_or_else(|| UiError::not_found("channel not found"))
}

async fn set_stream_record_preset(
    State(st): State<AppState>,
    Path(id): Path<u32>,
    Json(body): Json<PresetBody>,
) -> Result<Json<Value>, UiError> {
    st.orch
        .set_record_preset(id, body.preset.trim())
        .map_err(UiError::from)?;
    snapshot::stream_json(st.orch.as_ref(), st.ui.as_ref(), id)
        .map(Json)
        .ok_or_else(|| UiError::not_found("channel not found"))
}

async fn list_recordings(State(st): State<AppState>) -> Json<Value> {
    let mut out = Vec::new();
    for ch in st.orch.list_channels() {
        out.push(snapshot::recording_json(
            st.orch.as_ref(),
            st.ui.as_ref(),
            ch.id,
        ));
    }
    Json(Value::Array(out))
}

/// Start a REC role using the channel's UI name/category. Proxy files get a
/// `_proxy` suffix (see `Orchestrator::recording_file_name`).
fn start_role_recording(
    st: &AppState,
    id: u32,
    role: RecordingRole,
) -> Result<Json<Value>, UiError> {
    let meta = st.ui.rec_meta(id);
    let name = if meta.name.is_empty() {
        format!("ch{id}")
    } else {
        meta.name.clone()
    };
    let root = st.ui.recordings_dir();
    let cat = meta.category.clone();
    let dir = root.join(&cat);
    std::fs::create_dir_all(&dir).map_err(UiError::from)?;
    let stamp = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let safe = name.replace(' ', "_");
    let path = dir
        .join(st.orch.recording_file_name(id, role, &safe, &stamp))
        .to_string_lossy()
        .into_owned();
    let started = match role {
        RecordingRole::Proxy => st
            .orch
            .start_proxy_recording(id, Some(path), Some(name), Some(cat)),
        RecordingRole::Hq => st
            .orch
            .start_hq_recording(id, Some(path), Some(name), Some(cat)),
    };
    started.map_err(UiError::from)?;
    st.ui.mark_recording_started_role(id, role);
    Ok(Json(snapshot::recording_json(
        st.orch.as_ref(),
        st.ui.as_ref(),
        id,
    )))
}

fn stop_role_recording(
    st: &AppState,
    id: u32,
    role: RecordingRole,
) -> Result<Json<Value>, UiError> {
    let stopped = match role {
        RecordingRole::Proxy => st.orch.stop_proxy_recording(id),
        RecordingRole::Hq => st.orch.stop_hq_recording(id),
    };
    stopped.map_err(UiError::from)?;
    st.ui.mark_recording_stopped_role(id, role);
    Ok(Json(snapshot::recording_json(
        st.orch.as_ref(),
        st.ui.as_ref(),
        id,
    )))
}

/// `POST /api/recordings/{id}/start` — alias for HQ start.
async fn start_recording(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<Value>, UiError> {
    start_role_recording(&st, id, RecordingRole::Hq)
}

/// `POST /api/recordings/{id}/stop` — alias for HQ stop.
async fn stop_recording(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<Value>, UiError> {
    stop_role_recording(&st, id, RecordingRole::Hq)
}

async fn start_proxy_recording(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<Value>, UiError> {
    start_role_recording(&st, id, RecordingRole::Proxy)
}

async fn stop_proxy_recording(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<Value>, UiError> {
    stop_role_recording(&st, id, RecordingRole::Proxy)
}

async fn start_hq_recording(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<Value>, UiError> {
    start_role_recording(&st, id, RecordingRole::Hq)
}

async fn stop_hq_recording(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<Value>, UiError> {
    stop_role_recording(&st, id, RecordingRole::Hq)
}

#[derive(Deserialize)]
struct NameBody {
    name: String,
}

async fn set_rec_name(
    State(st): State<AppState>,
    Path(id): Path<u32>,
    Json(body): Json<NameBody>,
) -> Json<Value> {
    st.ui.set_rec_name(id, body.name);
    Json(snapshot::recording_json(
        st.orch.as_ref(),
        st.ui.as_ref(),
        id,
    ))
}

#[derive(Deserialize)]
struct CategoryBody {
    category: String,
}

async fn set_rec_category(
    State(st): State<AppState>,
    Path(id): Path<u32>,
    Json(body): Json<CategoryBody>,
) -> Result<Json<Value>, UiError> {
    if body.category.trim().is_empty() {
        return Err(UiError::bad("category required"));
    }
    st.ui.set_rec_category(id, body.category);
    Ok(Json(snapshot::recording_json(
        st.orch.as_ref(),
        st.ui.as_ref(),
        id,
    )))
}

#[derive(Deserialize)]
struct ScheduleBody {
    start_at: DateTime<Utc>,
    stop_at: DateTime<Utc>,
    #[serde(default)]
    arm_proxy: bool,
    #[serde(default = "default_arm_hq")]
    arm_hq: bool,
}

fn default_arm_hq() -> bool {
    true
}

async fn set_schedule(
    State(st): State<AppState>,
    Path(id): Path<u32>,
    Json(body): Json<ScheduleBody>,
) -> Result<Json<Value>, UiError> {
    if body.stop_at <= body.start_at {
        return Err(UiError::bad("stop_at must be after start_at"));
    }
    st.ui
        .set_schedule(id, body.start_at, body.stop_at, body.arm_proxy, body.arm_hq);
    Ok(Json(snapshot::recording_json(
        st.orch.as_ref(),
        st.ui.as_ref(),
        id,
    )))
}

async fn clear_schedule(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Json<Value> {
    st.ui.clear_schedule(id);
    Json(snapshot::recording_json(
        st.orch.as_ref(),
        st.ui.as_ref(),
        id,
    ))
}

async fn list_srt(State(st): State<AppState>) -> Json<Value> {
    let mut out = Vec::new();
    for ch in st.orch.list_channels() {
        out.push(snapshot::srt_json(st.orch.as_ref(), st.ui.as_ref(), ch.id));
    }
    Json(Value::Array(out))
}

async fn get_srt(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Json<Value> {
    Json(snapshot::srt_json(st.orch.as_ref(), st.ui.as_ref(), id))
}

#[derive(Deserialize)]
struct SrtUpdate {
    mode: Option<String>,
    port: Option<u16>,
    target: Option<String>,
    passphrase: Option<String>,
    latency_ms: Option<u32>,
}

async fn put_srt(
    State(st): State<AppState>,
    Path(id): Path<u32>,
    Json(body): Json<SrtUpdate>,
) -> Json<Value> {
    let mut s = st.ui.srt(id);
    if let Some(m) = body.mode {
        s.mode = m;
    }
    if let Some(p) = body.port {
        s.port = p;
    }
    if let Some(t) = body.target {
        s.target = t;
    }
    if let Some(p) = body.passphrase {
        s.passphrase = p;
    }
    if let Some(l) = body.latency_ms {
        s.latency_ms = l;
    }
    st.ui.set_srt(id, s);
    Json(snapshot::srt_json(st.orch.as_ref(), st.ui.as_ref(), id))
}

async fn start_srt(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<Value>, UiError> {
    let url = st.ui.srt_output_url(id).map_err(UiError::from)?;
    st.orch
        .start_srt(id, Some(url))
        .map_err(UiError::from)?;
    Ok(Json(snapshot::srt_json(
        st.orch.as_ref(),
        st.ui.as_ref(),
        id,
    )))
}

async fn stop_srt(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<Value>, UiError> {
    st.orch.stop_srt(id).map_err(UiError::from)?;
    Ok(Json(snapshot::srt_json(
        st.orch.as_ref(),
        st.ui.as_ref(),
        id,
    )))
}

async fn list_presets_ui(State(st): State<AppState>) -> Json<Value> {
    let presets: Vec<Value> = st
        .orch
        .list_presets()
        .iter()
        .map(|(id, p)| {
            json!({
                "id": id,
                "label": p.label,
                "video_codec": p.video_codec,
                "video_bitrate": p.video_bitrate,
                "video_maxrate": p.video_maxrate.clone().unwrap_or_default(),
                "video_bufsize": p.video_bufsize.clone().unwrap_or_default(),
                "video_preset": p.video_preset,
                "video_gop": p.video_gop,
                "audio_bitrate": p.audio_bitrate,
                "audio_channels": p.audio_channels,
            })
        })
        .collect();
    Json(Value::Array(presets))
}

#[derive(Deserialize)]
struct PresetDef {
    id: Option<String>,
    label: Option<String>,
    video_codec: Option<String>,
    video_bitrate: String,
    video_maxrate: Option<String>,
    video_bufsize: Option<String>,
    video_preset: Option<String>,
    video_gop: Option<u32>,
    audio_bitrate: Option<String>,
    audio_channels: Option<u32>,
}

fn to_preset(id: &str, body: PresetDef) -> roc_config::EncodePreset {
    roc_config::EncodePreset {
        label: body
            .label
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| id.to_string()),
        video_codec: body.video_codec.unwrap_or_default(),
        video_bitrate: body.video_bitrate,
        video_maxrate: body.video_maxrate.filter(|s| !s.trim().is_empty()),
        video_bufsize: body.video_bufsize.filter(|s| !s.trim().is_empty()),
        video_preset: body.video_preset.unwrap_or_default(),
        video_gop: body.video_gop.unwrap_or(0),
        audio_bitrate: body.audio_bitrate.unwrap_or_default(),
        audio_channels: body.audio_channels.unwrap_or(0),
    }
}

async fn create_preset_ui(
    State(st): State<AppState>,
    Json(body): Json<PresetDef>,
) -> Result<Json<Value>, UiError> {
    let id = body
        .id
        .clone()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| UiError::bad("id is required"))?;
    let saved = st
        .orch
        .upsert_preset(&id, to_preset(&id, body), true)
        .map_err(UiError::from)?;
    Ok(Json(json!({
        "id": id,
        "label": saved.label,
        "video_codec": saved.video_codec,
        "video_bitrate": saved.video_bitrate,
        "video_maxrate": saved.video_maxrate.unwrap_or_default(),
        "video_bufsize": saved.video_bufsize.unwrap_or_default(),
        "video_preset": saved.video_preset,
        "video_gop": saved.video_gop,
        "audio_bitrate": saved.audio_bitrate,
        "audio_channels": saved.audio_channels,
    })))
}

async fn upsert_preset_ui(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PresetDef>,
) -> Result<Json<Value>, UiError> {
    let saved = st
        .orch
        .upsert_preset(&id, to_preset(&id, body), false)
        .map_err(UiError::from)?;
    Ok(Json(json!({
        "id": id,
        "label": saved.label,
        "video_codec": saved.video_codec,
        "video_bitrate": saved.video_bitrate,
        "video_maxrate": saved.video_maxrate.unwrap_or_default(),
        "video_bufsize": saved.video_bufsize.unwrap_or_default(),
        "video_preset": saved.video_preset,
        "video_gop": saved.video_gop,
        "audio_bitrate": saved.audio_bitrate,
        "audio_channels": saved.audio_channels,
    })))
}

async fn delete_preset_ui(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, UiError> {
    st.orch.delete_preset(&id).map_err(UiError::from)?;
    Ok(Json(json!({ "status": "deleted", "id": id })))
}

async fn encode_options() -> Json<Value> {
    Json(json!({
        "codecs": [
            {
                "id": "h264_nvenc",
                "label": "H.264 NVENC",
                "presets": [
                    {"id": "p1", "label": "P1 fastest"},
                    {"id": "p4", "label": "P4 HQ"},
                    {"id": "llhq", "label": "Low-latency HQ"}
                ]
            },
            {
                "id": "hevc_nvenc",
                "label": "HEVC NVENC",
                "presets": [
                    {"id": "p4", "label": "P4 HQ"},
                    {"id": "llhq", "label": "Low-latency HQ"}
                ]
            },
            {
                "id": "avenc_dnxhd",
                "label": "DNxHD (mezz REC)",
                "presets": [
                    {"id": "sq", "label": "SQ — 8-bit (120 @ i50 / 240 @ p50)"},
                    {"id": "hq", "label": "HQ — 8-bit (185 @ i50 / 365 @ p50)"},
                    {"id": "hqx", "label": "HQX — 10-bit (i50/p50; requires 10-bit source)"}
                ]
            },
            {
                "id": "xavc_intra",
                "label": "XAVC Intra HD (mezz REC)",
                "presets": [
                    {"id": "intra", "label": "Intra"}
                ]
            }
        ]
    }))
}

async fn list_playout_ui(State(st): State<AppState>) -> Json<Value> {
    Json(Value::Array(snapshot::playout_json(
        st.orch.as_ref(),
        st.ui.as_ref(),
    )))
}

fn playout_client_json(st: &AppState, id: u32) -> Result<Value, UiError> {
    snapshot::playout_json(st.orch.as_ref(), st.ui.as_ref())
        .into_iter()
        .find(|v| v.get("id").and_then(|x| x.as_u64()) == Some(id as u64))
        .ok_or_else(|| UiError::not_found("playout not found"))
}

async fn get_playout_ui(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<Value>, UiError> {
    Ok(Json(playout_client_json(&st, id)?))
}

#[derive(Deserialize)]
struct PlayoutUpdateBody {
    name: Option<String>,
    format_code: Option<String>,
    source: Option<String>,
    file_id: Option<String>,
    #[serde(rename = "loop")]
    loop_file: Option<bool>,
    mark_in_sec: Option<f64>,
    /// JSON `null` clears the out point; omit to leave unchanged.
    #[serde(default, deserialize_with = "deserialize_optional_mark_out")]
    mark_out_sec: Option<Option<f64>>,
    mode: Option<String>,
    port: Option<u16>,
    target: Option<String>,
    passphrase: Option<String>,
    latency_ms: Option<u32>,
}

fn deserialize_optional_mark_out<'de, D>(deserializer: D) -> Result<Option<Option<f64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<f64>::deserialize(deserializer)?))
}

fn normalize_marks(meta: &mut crate::ui::state::PlayoutMeta) {
    if !meta.mark_in_sec.is_finite() || meta.mark_in_sec < 0.0 {
        meta.mark_in_sec = 0.0;
    }
    if let Some(dur) = meta.duration_sec {
        if dur.is_finite() && dur > 0.0 {
            meta.mark_in_sec = meta.mark_in_sec.min(dur);
        }
    }
    if let Some(out) = meta.mark_out_sec {
        if !out.is_finite() || out <= meta.mark_in_sec {
            meta.mark_out_sec = None;
        } else if let Some(dur) = meta.duration_sec {
            if dur.is_finite() && out > dur {
                meta.mark_out_sec = Some(dur);
            }
        }
    }
}

fn refresh_file_duration(st: &AppState, meta: &mut crate::ui::state::PlayoutMeta) {
    if meta.source != "file" || meta.file_id.trim().is_empty() {
        return;
    }
    let Ok(path) = resolve_playout_file_path(st, &meta.file_id) else {
        return;
    };
    if let Some(dur) = roc_pipelines::probe_file_duration(&path, 2500) {
        meta.duration_sec = Some(dur);
        normalize_marks(meta);
    }
    let audio = roc_pipelines::probe_playout_audio(&path);
    meta.video_codec = audio.video_codec;
    meta.audio_codec = audio.audio_codec;
    meta.audio_tracks = audio.audio_tracks as u32;
    meta.audio_channels = audio.audio_channels;
}

fn apply_playout_file_control(st: &AppState, id: u32, meta: &crate::ui::state::PlayoutMeta) {
    let client_id = format!("decode-{id}");
    let _ = st.orch.set_playout_file_control(
        &client_id,
        roc_pipelines::PlayoutFileControl {
            loop_file: meta.loop_file,
            mark_in_sec: meta.mark_in_sec,
            mark_out_sec: meta.mark_out_sec,
        },
    );
}

async fn put_playout_ui(
    State(st): State<AppState>,
    Path(id): Path<u32>,
    Json(body): Json<PlayoutUpdateBody>,
) -> Result<Json<Value>, UiError> {
    let client_id = format!("decode-{id}");
    if !st.orch.cfg.playout.iter().any(|c| c.id == client_id) {
        return Err(UiError::not_found("playout not found"));
    }
    let cfg_name = st
        .orch
        .cfg
        .playout
        .iter()
        .find(|c| c.id == client_id)
        .map(|c| c.name.clone())
        .unwrap_or_else(|| format!("Decode {id}"));
    st.ui.ensure_playout(id, &cfg_name);
    let mut meta = st.ui.playout(id);
    let mut file_changed = false;
    if let Some(n) = body.name {
        meta.name = n;
    }
    if let Some(fc) = body.format_code {
        meta.format_code = if fc.trim().is_empty() {
            "auto".into()
        } else {
            fc
        };
    }
    if let Some(s) = body.source {
        meta.source = if s == "file" { "file".into() } else { "srt".into() };
    }
    if let Some(f) = body.file_id {
        if f != meta.file_id {
            file_changed = true;
            meta.duration_sec = None;
            meta.video_codec.clear();
            meta.audio_codec.clear();
            meta.audio_tracks = 0;
            meta.audio_channels = 0;
            meta.mark_in_sec = 0.0;
            meta.mark_out_sec = None;
        }
        meta.file_id = f;
    }
    if let Some(l) = body.loop_file {
        meta.loop_file = l;
    }
    if let Some(v) = body.mark_in_sec {
        meta.mark_in_sec = v;
    }
    if let Some(out) = body.mark_out_sec {
        meta.mark_out_sec = out;
    }
    if let Some(m) = body.mode {
        meta.mode = if m == "listener" {
            "listener".into()
        } else {
            "caller".into()
        };
    }
    if let Some(p) = body.port {
        meta.port = p;
    }
    if let Some(t) = body.target {
        meta.target = t;
    }
    if let Some(p) = body.passphrase {
        meta.passphrase = p;
    }
    if let Some(l) = body.latency_ms {
        meta.latency_ms = l;
    }
    if file_changed || meta.source == "file" {
        refresh_file_duration(&st, &mut meta);
    } else {
        normalize_marks(&mut meta);
    }
    st.ui.set_playout(id, meta.clone());
    apply_playout_file_control(&st, id, &meta);
    Ok(Json(playout_client_json(&st, id)?))
}

#[derive(Deserialize)]
struct PlayoutStartBody {
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    format_code: Option<String>,
}

fn resolve_playout_file_path(st: &AppState, file_id: &str) -> Result<String, UiError> {
    let file_id = file_id.trim();
    if file_id.is_empty() {
        return Err(UiError::bad("file_id required for file source"));
    }
    if let Some(rest) = file_id.strip_prefix("lib:") {
        let decoded = percent_decode_simple(rest);
        let (cat, name) = decoded
            .split_once('/')
            .ok_or_else(|| UiError::bad("invalid library file ref"))?;
        let path = library::file_path(&st.ui.recordings_dir(), cat, name).map_err(UiError::from)?;
        return Ok(path.to_string_lossy().into_owned());
    }
    if let Some(path) = st.playout_media.path_for(file_id) {
        return Ok(path.to_string_lossy().into_owned());
    }
    // Absolute path passthrough for advanced use.
    if PathBuf::from(file_id).is_file() {
        return Ok(file_id.to_string());
    }
    Err(UiError::bad(format!("media `{file_id}` not found")))
}

fn percent_decode_simple(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = match bytes[i + 1] {
                b'0'..=b'9' => bytes[i + 1] - b'0',
                b'a'..=b'f' => bytes[i + 1] - b'a' + 10,
                b'A'..=b'F' => bytes[i + 1] - b'A' + 10,
                _ => {
                    out.push(bytes[i]);
                    i += 1;
                    continue;
                }
            };
            let l = match bytes[i + 2] {
                b'0'..=b'9' => bytes[i + 2] - b'0',
                b'a'..=b'f' => bytes[i + 2] - b'a' + 10,
                b'A'..=b'F' => bytes[i + 2] - b'A' + 10,
                _ => {
                    out.push(bytes[i]);
                    i += 1;
                    continue;
                }
            };
            out.push((h << 4) | l);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn resolve_playout_source(st: &AppState, id: u32, body: &PlayoutStartBody) -> Result<(String, Option<String>), UiError> {
    if let Some(s) = body
        .source
        .as_ref()
        .or(body.target.as_ref())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        return Ok((s, body.format_code.clone()));
    }
    let client_id = format!("decode-{id}");
    let cfg_name = st
        .orch
        .cfg
        .playout
        .iter()
        .find(|c| c.id == client_id)
        .map(|c| c.name.clone())
        .unwrap_or_else(|| format!("Decode {id}"));
    st.ui.ensure_playout(id, &cfg_name);
    let meta = st.ui.playout(id);
    let format_code = body
        .format_code
        .clone()
        .or_else(|| {
            if meta.format_code.is_empty() {
                Some("auto".into())
            } else {
                Some(meta.format_code.clone())
            }
        });
    if meta.source == "file" {
        let path = resolve_playout_file_path(st, &meta.file_id)?;
        Ok((path, format_code))
    } else {
        let url = st.ui.playout_srt_url(id).map_err(UiError::from)?;
        Ok((url, format_code))
    }
}

async fn start_playout_ui(
    State(st): State<AppState>,
    Path(id): Path<u32>,
    body: Result<Json<PlayoutStartBody>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, UiError> {
    if st.ui.workflow_mode(id) == "tc" || st.ui.tc(id).enabled {
        return Err(UiError::bad("stop TC burn-in before starting decode"));
    }
    let body = body.ok().map(|j| j.0).unwrap_or(PlayoutStartBody {
        source: None,
        target: None,
        format_code: None,
    });
    let client_id = format!("decode-{id}");
    let (source, format_code) = resolve_playout_source(&st, id, &body)?;
    st.orch
        .start_playout(&client_id, source, format_code)
        .map_err(UiError::from)?;
    let mut meta = st.ui.playout(id);
    if meta.source == "file" {
        refresh_file_duration(&st, &mut meta);
        st.ui.set_playout(id, meta.clone());
        apply_playout_file_control(&st, id, &meta);
        if meta.mark_in_sec > 0.05 {
            let _ = st.orch.seek_playout(&client_id, meta.mark_in_sec);
        }
    }
    Ok(Json(playout_client_json(&st, id)?))
}

async fn stop_playout_ui(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<Value>, UiError> {
    let client_id = format!("decode-{id}");
    st.orch.stop_playout(&client_id).map_err(UiError::from)?;
    Ok(Json(playout_client_json(&st, id)?))
}

async fn pause_playout_ui(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<Value>, UiError> {
    let client_id = format!("decode-{id}");
    st.orch.pause_playout(&client_id).map_err(UiError::from)?;
    Ok(Json(playout_client_json(&st, id)?))
}

async fn resume_playout_ui(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<Value>, UiError> {
    let client_id = format!("decode-{id}");
    st.orch.resume_playout(&client_id).map_err(UiError::from)?;
    Ok(Json(playout_client_json(&st, id)?))
}

#[derive(Deserialize)]
struct PlayoutSeekBody {
    position_sec: f64,
}

async fn seek_playout_ui(
    State(st): State<AppState>,
    Path(id): Path<u32>,
    Json(body): Json<PlayoutSeekBody>,
) -> Result<Json<Value>, UiError> {
    let client_id = format!("decode-{id}");
    st.orch
        .seek_playout(&client_id, body.position_sec)
        .map_err(UiError::from)?;
    Ok(Json(playout_client_json(&st, id)?))
}

async fn playout_logs_ui(Path(id): Path<u32>) -> Json<Value> {
    Json(json!({
        "id": id,
        "lines": [
            format!("playout decode-{id}: logs not yet wired to GST bus"),
        ],
    }))
}

fn playout_format_list() -> Vec<Value> {
    vec![
        json!({"code": "auto", "label": "Auto (from source)", "width": 1920, "height": 1080, "fps": 50, "interlaced": false}),
        json!({"code": "Hp50", "label": "1080p50", "width": 1920, "height": 1080, "fps": 50, "interlaced": false}),
        json!({"code": "Hi50", "label": "1080i50 → 1080p50 OUT", "width": 1920, "height": 1080, "fps": 25, "interlaced": true}),
        json!({"code": "Hp25", "label": "1080p25", "width": 1920, "height": 1080, "fps": 25, "interlaced": false}),
        json!({"code": "Hp59.94", "label": "1080p59.94", "width": 1920, "height": 1080, "fps": 59.94, "interlaced": false}),
        json!({"code": "Hi59.94", "label": "1080i59.94 → 1080p59.94 OUT", "width": 1920, "height": 1080, "fps": 29.97, "interlaced": true}),
    ]
}

async fn playout_devices(State(st): State<AppState>) -> Json<Value> {
    let formats = playout_format_list();
    let mut devices = Vec::new();
    if let Ok(report) = st.orch.probe() {
        for d in report.devices {
            if matches!(
                d.direction,
                roc_devices::DeviceDirection::Output | roc_devices::DeviceDirection::Unknown
            ) {
                devices.push(json!({
                    "name": d.name,
                    "label": d.name,
                    "open_name": d.name,
                    "formats": formats.clone(),
                    "probe_log": report.notes.join("\n"),
                }));
            }
        }
        // Deduplicate by name (probe may list in+out with same label).
        let mut seen = std::collections::HashSet::new();
        devices.retain(|d| {
            let name = d.get("name").and_then(|n| n.as_str()).unwrap_or("");
            seen.insert(name.to_string())
        });
    }
    if devices.is_empty() {
        for p in &st.orch.cfg.playout {
            devices.push(json!({
                "name": p.device,
                "label": p.device,
                "open_name": p.device,
                "formats": formats.clone(),
                "probe_log": "",
            }));
        }
    }
    Json(Value::Array(devices))
}

async fn lib_categories(State(st): State<AppState>) -> Result<Json<Value>, UiError> {
    let root = st.ui.recordings_dir();
    Ok(Json(Value::Array(
        library::list_categories(&root).map_err(UiError::from)?,
    )))
}

#[derive(Deserialize)]
struct CatName {
    name: String,
}

async fn lib_create_cat(
    State(st): State<AppState>,
    Json(body): Json<CatName>,
) -> Result<Json<Value>, UiError> {
    let root = st.ui.recordings_dir();
    Ok(Json(
        library::create_category(&root, &body.name).map_err(UiError::from)?,
    ))
}

async fn lib_rename_cat(
    State(st): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<CatName>,
) -> Result<Json<Value>, UiError> {
    let root = st.ui.recordings_dir();
    Ok(Json(
        library::rename_category(&root, &name, &body.name).map_err(UiError::from)?,
    ))
}

async fn lib_delete_cat(
    State(st): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Value>, UiError> {
    let root = st.ui.recordings_dir();
    library::delete_category(&root, &name).map_err(UiError::from)?;
    Ok(Json(json!({ "status": "deleted" })))
}

#[derive(Deserialize)]
struct FilesQuery {
    category: Option<String>,
}

async fn lib_files(
    State(st): State<AppState>,
    Query(q): Query<FilesQuery>,
) -> Result<Json<Value>, UiError> {
    let root = st.ui.recordings_dir();
    Ok(Json(Value::Array(
        library::list_files(&root, q.category.as_deref().unwrap_or(""))
            .map_err(UiError::from)?,
    )))
}

async fn lib_file_get(
    State(st): State<AppState>,
    Path((category, name)): Path<(String, String)>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Response, UiError> {
    let root = st.ui.recordings_dir();
    let path = library::file_path(&root, &category, &name).map_err(UiError::from)?;
    let ctype = library::content_type_for(&path);
    let file = File::open(&path).await.map_err(UiError::from)?;
    let meta = tokio::fs::metadata(&path).await.map_err(UiError::from)?;
    let stream = ReaderStream::new(file);
    let body = axum::body::Body::from_stream(stream);
    let mut res = Response::new(body);
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(ctype),
    );
    res.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    if q.get("download").map(|s| s.as_str()) == Some("1") {
        let disp = format!("attachment; filename=\"{}\"", name.replace('"', ""));
        if let Ok(v) = header::HeaderValue::from_str(&disp) {
            res.headers_mut().insert(header::CONTENT_DISPOSITION, v);
        }
    }
    res.headers_mut().insert(
        header::CONTENT_LENGTH,
        header::HeaderValue::from_str(&meta.len().to_string()).unwrap(),
    );
    Ok(res)
}

async fn lib_file_delete(
    State(st): State<AppState>,
    Path((category, name)): Path<(String, String)>,
) -> Result<Json<Value>, UiError> {
    let root = st.ui.recordings_dir();
    library::delete_file(&root, &category, &name).map_err(UiError::from)?;
    Ok(Json(json!({ "status": "deleted" })))
}

#[derive(Deserialize)]
struct MoveBody {
    from_category: String,
    to_category: String,
    name: String,
}

async fn lib_move(
    State(st): State<AppState>,
    Json(body): Json<MoveBody>,
) -> Result<Json<Value>, UiError> {
    let root = st.ui.recordings_dir();
    Ok(Json(
        library::move_file(&root, &body.from_category, &body.to_category, &body.name)
            .map_err(UiError::from)?,
    ))
}

async fn get_rec_path(State(st): State<AppState>) -> Json<Value> {
    Json(json!({ "path": st.ui.recordings_dir().display().to_string() }))
}

#[derive(Deserialize)]
struct PathBody {
    path: String,
}

async fn set_rec_path(
    State(st): State<AppState>,
    Json(body): Json<PathBody>,
) -> Result<Json<Value>, UiError> {
    let p = st
        .ui
        .set_recordings_dir(PathBuf::from(body.path))
        .map_err(UiError::from)?;
    st.sys.set_disk_path(p.clone());
    Ok(Json(json!({ "path": p.display().to_string() })))
}

async fn system_status(State(st): State<AppState>) -> Json<Value> {
    let mut snap = st.sys.snapshot();
    // Keep disk path aligned with current recordings root.
    let rec = st.ui.recordings_dir();
    if snap.disk_path != rec.display().to_string() {
        st.sys.set_disk_path(rec.clone());
        snap = st.sys.snapshot();
    }
    let h = st.orch.health();
    let mut v = serde_json::to_value(&snap).unwrap_or_else(|_| json!({}));
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "nvenc_used".into(),
            h.get("nvenc_used").cloned().unwrap_or(json!(0)),
        );
        obj.insert(
            "nvenc_limit".into(),
            h.get("nvenc_limit").cloned().unwrap_or(json!(8)),
        );
        obj.insert("ok".into(), json!(true));
        obj.insert("backend".into(), json!("media_engine"));
    }
    Json(v)
}

async fn list_playout_media(State(st): State<AppState>) -> Json<Value> {
    let items: Vec<Value> = st
        .playout_media
        .list()
        .into_iter()
        .map(|it| {
            json!({
                "id": it.id,
                "name": it.name,
                "size": it.size,
                "created_at": it.created_at.to_rfc3339(),
            })
        })
        .collect();
    Json(Value::Array(items))
}

async fn upload_playout_media(
    State(st): State<AppState>,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<Value>, UiError> {
    let mut filename = None;
    let mut bytes: Option<Vec<u8>> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| UiError::bad(e.to_string()))?
    {
        if field.name() != Some("file") {
            continue;
        }
        filename = field.file_name().map(|s| s.to_string());
        bytes = Some(
            field
                .bytes()
                .await
                .map_err(|e| UiError::bad(e.to_string()))?
                .to_vec(),
        );
        break;
    }
    let name = filename.ok_or_else(|| UiError::bad("file field required"))?;
    let data = bytes.ok_or_else(|| UiError::bad("file field required"))?;
    let item = st
        .playout_media
        .add_from_reader(&name, data.as_slice())
        .map_err(UiError::from)?;
    Ok(Json(json!({
        "id": item.id,
        "name": item.name,
        "size": item.size,
        "created_at": item.created_at.to_rfc3339(),
    })))
}

async fn delete_playout_media(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, UiError> {
    st.playout_media.delete(&id).map_err(UiError::from)?;
    Ok(Json(json!({ "status": "deleted" })))
}

async fn workflows_map(State(st): State<AppState>) -> Json<Value> {
    let mut m = serde_json::Map::new();
    for ch in st.orch.list_channels() {
        m.insert(
            ch.id.to_string(),
            json!({ "mode": st.ui.workflow_mode(ch.id) }),
        );
    }
    Json(Value::Object(m))
}

#[derive(Deserialize)]
struct WfBody {
    #[serde(default)]
    mode: Option<String>,
    // Legacy boolean fields — ignored if `mode` is set.
    #[serde(default)]
    pair: Option<bool>,
    #[serde(default)]
    tc: Option<bool>,
    #[serde(default)]
    commentator: Option<bool>,
}

async fn set_workflow_ui(
    State(st): State<AppState>,
    Path(id): Path<u32>,
    Json(body): Json<WfBody>,
) -> Result<Json<Value>, UiError> {
    let prev = st.ui.workflow_mode(id);
    let mode = body
        .mode
        .clone()
        .or_else(|| {
            if body.tc == Some(true) {
                Some("tc".into())
            } else if body.commentator == Some(true) {
                Some("remote_commentator".into())
            } else if body.pair == Some(true) {
                Some("pair".into())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "pair".into());
    let mode = match mode.as_str() {
        "tc" => "tc",
        "remote_commentator" => "remote_commentator",
        _ => "pair",
    };

    if prev != mode {
        if prev == "tc" {
            let _ = tc::stop_tc(st.orch.as_ref(), st.ui.as_ref(), id);
        }
        if mode == "tc" {
            st.ui.set_workflow_mode(id, mode);
            let hls = st.hls_dir.to_string_lossy().to_string();
            let _ = tc::start_tc(st.orch.as_ref(), st.ui.as_ref(), id, &hls);
        } else {
            st.ui.set_workflow_mode(id, mode);
            if mode == "pair" && st.ui.encode_wanted(id).unwrap_or(true) {
                let _ = st.orch.start_capture(id);
            }
        }
    } else {
        st.ui.set_workflow_mode(id, mode);
    }

    Ok(Json(json!({ "id": id, "mode": st.ui.workflow_mode(id) })))
}

#[derive(Deserialize)]
struct TcLoopBody {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    udp_port: Option<u16>,
    #[serde(default)]
    fontsize: Option<u32>,
    #[serde(default)]
    opacity: Option<f64>,
    #[serde(default)]
    position: Option<String>,
    #[serde(default)]
    x: Option<f64>,
    #[serde(default)]
    y: Option<f64>,
}

async fn get_tc_loop(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<Value>, UiError> {
    if st.orch.channel_config(id).is_err() {
        return Err(UiError::not_found("channel not found"));
    }
    let meta = st.ui.tc(id);
    let live = st.orch.tc_loop_snapshot(id);
    Ok(Json(tc::tc_info_json(id, &meta, live.as_ref())))
}

async fn put_tc_loop(
    State(st): State<AppState>,
    Path(id): Path<u32>,
    Json(body): Json<TcLoopBody>,
) -> Result<Json<Value>, UiError> {
    if st.orch.channel_config(id).is_err() {
        return Err(UiError::not_found("channel not found"));
    }
    let mut meta = st.ui.tc(id);
    let prev_source = meta.source.clone();
    let prev_udp = meta.udp_port;
    if let Some(ref s) = body.source {
        meta.source = if s.eq_ignore_ascii_case("external") {
            "external".into()
        } else {
            "tod".into()
        };
    }
    if let Some(p) = body.udp_port {
        meta.udp_port = p;
    }
    if let Some(f) = body.fontsize {
        meta.fontsize = f.clamp(12, 200);
    }
    if let Some(o) = body.opacity {
        meta.opacity = o.clamp(0.15, 1.0);
    }
    // Freeform x/y take precedence; legacy position snaps set x/y.
    let mut xy_set = false;
    if let Some(x) = body.x {
        meta.x = x.clamp(0.0, 1.0);
        xy_set = true;
    }
    if let Some(y) = body.y {
        meta.y = y.clamp(0.0, 1.0);
        xy_set = true;
    }
    if let Some(p) = body.position {
        meta.position = match p.as_str() {
            "bottom_right" | "bottom_left" | "top_right" | "top_left" | "center" => p.clone(),
            _ => "top_left".into(),
        };
        if !xy_set {
            let (x, y) = roc_pipelines::TcLoopPosition::parse(&meta.position).default_xy();
            meta.x = x;
            meta.y = y;
        }
    }
    if xy_set {
        meta.position = roc_pipelines::TcLoopPosition::nearest(meta.x, meta.y)
            .as_str()
            .into();
    }

    let want = body.enabled.unwrap_or(meta.enabled);
    let was = meta.enabled;
    // Only actual source/UDP value changes need a full TC pipeline relaunch.
    let need_relaunch = (body.source.is_some() && meta.source != prev_source)
        || (body.udp_port.is_some() && meta.udp_port != prev_udp);
    meta.enabled = want;
    st.ui.set_tc(id, meta);

    let hls = st.hls_dir.to_string_lossy().to_string();
    let live_running = st
        .orch
        .tc_loop_snapshot(id)
        .as_ref()
        .map(|s| {
            matches!(
                s.status,
                roc_pipelines::TcLoopStatus::Running | roc_pipelines::TcLoopStatus::Restarting
            )
        })
        .unwrap_or(false);

    let out = if !want {
        if was || live_running {
            tc::stop_tc(st.orch.as_ref(), st.ui.as_ref(), id).map_err(UiError::from)?
        } else {
            tc::tc_info_json(id, &st.ui.tc(id), None)
        }
    } else if !live_running || need_relaunch {
        st.ui.set_workflow_mode(id, "tc");
        tc::start_tc(st.orch.as_ref(), st.ui.as_ref(), id, &hls).map_err(UiError::from)?
    } else {
        let m = st.ui.tc(id);
        let (x, y) = m.resolved_xy();
        let snap = st
            .orch
            .update_tc_overlay(
                id,
                m.fontsize,
                m.opacity,
                x,
                y,
                roc_pipelines::TcLoopPosition::nearest(x, y),
            )
            .map_err(UiError::from)?;
        tc::tc_info_json(id, &m, Some(&snap))
    };
    Ok(Json(out))
}

async fn thumb(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Response, UiError> {
    let dir = st.hls_dir.join(id.to_string());
    let path = newest_thumb(&dir).ok_or_else(|| UiError::not_found("thumb not found"))?;
    let bytes = tokio::fs::read(&path).await.map_err(UiError::from)?;
    Ok((
        [(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "no-store")],
        bytes,
    )
        .into_response())
}

async fn thumb_playout(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Response, UiError> {
    let dir = st.hls_dir.join("playout").join(id.to_string());
    let path = newest_thumb(&dir).ok_or_else(|| UiError::not_found("thumb not found"))?;
    let bytes = tokio::fs::read(&path).await.map_err(UiError::from)?;
    Ok((
        [(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "no-store")],
        bytes,
    )
        .into_response())
}

fn newest_thumb(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let fixed = dir.join("thumb.jpg");
    if fixed.is_file() {
        return Some(fixed);
    }
    let mut best: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    let rd = std::fs::read_dir(dir).ok()?;
    for ent in rd.flatten() {
        let name = ent.file_name();
        let n = name.to_string_lossy();
        if !(n.starts_with("thumb") && n.ends_with(".jpg")) {
            continue;
        }
        let meta = ent.metadata().ok()?;
        let modified = meta.modified().ok()?;
        if best.as_ref().map(|(t, _)| modified > *t).unwrap_or(true) {
            best = Some((modified, ent.path()));
        }
    }
    best.map(|(_, p)| p)
}

pub struct UiError {
    status: StatusCode,
    message: String,
}

impl UiError {
    fn bad(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: msg.into(),
        }
    }
    fn not_found(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: msg.into(),
        }
    }
}

impl From<anyhow::Error> for UiError {
    fn from(value: anyhow::Error) -> Self {
        let message = format!("{value:#}");
        let status = if message.contains("not found") {
            StatusCode::NOT_FOUND
        } else if message.contains("already") || message.contains("cannot") {
            StatusCode::CONFLICT
        } else {
            StatusCode::BAD_REQUEST
        };
        Self { status, message }
    }
}

impl From<std::io::Error> for UiError {
    fn from(value: std::io::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: value.to_string(),
        }
    }
}

impl IntoResponse for UiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "error": self.message })),
        )
            .into_response()
    }
}
