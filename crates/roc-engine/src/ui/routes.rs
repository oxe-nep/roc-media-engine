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
use tokio_util::io::ReaderStream;

use crate::ui::library;
use crate::ui::snapshot;
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
        .route("/api/recordings/{id}/start", post(start_recording))
        .route("/api/recordings/{id}/stop", post(stop_recording))
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
        .route("/api/playout/{id}/start", post(start_playout_ui))
        .route("/api/playout/{id}/stop", post(stop_playout_ui))
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
    st.orch.start_capture(id).map_err(UiError::from)?;
    snapshot::stream_json(st.orch.as_ref(), st.ui.as_ref(), id)
        .map(Json)
        .ok_or_else(|| UiError::not_found("channel not found"))
}

async fn stop_stream(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<Value>, UiError> {
    let _ = st.orch.stop_srt(id);
    let _ = st.orch.stop_recording(id);
    st.ui.mark_recording_stopped(id);
    st.orch.stop_capture(id).map_err(UiError::from)?;
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

async fn start_recording(
    State(st): State<AppState>,
    Path(id): Path<u32>,
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
    let stamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let safe = name.replace(' ', "_");
    let ext = st.orch.recording_ext(id);
    let path = dir
        .join(format!("{safe}_ch{id}_{stamp}.{ext}"))
        .to_string_lossy()
        .into_owned();
    st.orch
        .start_recording(id, Some(path), Some(name), Some(cat))
        .map_err(UiError::from)?;
    st.ui.mark_recording_started(id);
    Ok(Json(snapshot::recording_json(
        st.orch.as_ref(),
        st.ui.as_ref(),
        id,
    )))
}

async fn stop_recording(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<Value>, UiError> {
    st.orch.stop_recording(id).map_err(UiError::from)?;
    st.ui.mark_recording_stopped(id);
    Ok(Json(snapshot::recording_json(
        st.orch.as_ref(),
        st.ui.as_ref(),
        id,
    )))
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
}

async fn set_schedule(
    State(st): State<AppState>,
    Path(id): Path<u32>,
    Json(body): Json<ScheduleBody>,
) -> Result<Json<Value>, UiError> {
    if body.stop_at <= body.start_at {
        return Err(UiError::bad("stop_at must be after start_at"));
    }
    st.ui.set_schedule(id, body.start_at, body.stop_at);
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
                    {"id": "dnxhd", "label": "DNxHD"}
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
    Json(Value::Array(snapshot::playout_json(st.orch.as_ref())))
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

async fn start_playout_ui(
    State(st): State<AppState>,
    Path(id): Path<u32>,
    body: Result<Json<PlayoutStartBody>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, UiError> {
    let body = body.ok().map(|j| j.0).unwrap_or(PlayoutStartBody {
        source: None,
        target: None,
        format_code: None,
    });
    let client_id = format!("decode-{id}");
    let source = body
        .source
        .or(body.target)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| UiError::bad("source/target required"))?;
    st.orch
        .start_playout(&client_id, source, body.format_code)
        .map_err(UiError::from)?;
    let list = snapshot::playout_json(st.orch.as_ref());
    list.into_iter()
        .find(|v| v.get("id").and_then(|x| x.as_u64()) == Some(id as u64))
        .map(Json)
        .ok_or_else(|| UiError::not_found("playout not found"))
}

async fn stop_playout_ui(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<Value>, UiError> {
    let client_id = format!("decode-{id}");
    st.orch.stop_playout(&client_id).map_err(UiError::from)?;
    let list = snapshot::playout_json(st.orch.as_ref());
    list.into_iter()
        .find(|v| v.get("id").and_then(|x| x.as_u64()) == Some(id as u64))
        .map(Json)
        .ok_or_else(|| UiError::not_found("playout not found"))
}

async fn playout_devices(State(st): State<AppState>) -> Json<Value> {
    let mut devices = Vec::new();
    if let Ok(report) = st.orch.probe() {
        // Best-effort: serialize whatever probe returns.
        if let Ok(v) = serde_json::to_value(&report) {
            if let Some(arr) = v.get("decklink").and_then(|d| d.as_array()) {
                for (i, d) in arr.iter().enumerate() {
                    let name = d
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("DeckLink");
                    devices.push(json!({
                        "id": name,
                        "label": name,
                        "index": i,
                    }));
                }
            }
        }
    }
    if devices.is_empty() {
        for p in &st.orch.cfg.playout {
            devices.push(json!({
                "id": p.device,
                "label": p.device,
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
            json!({ "pair": false, "tc": false, "commentator": false }),
        );
    }
    Json(Value::Object(m))
}

#[derive(Deserialize)]
struct WfBody {
    #[serde(default)]
    pair: Option<bool>,
    #[serde(default)]
    tc: Option<bool>,
    #[serde(default)]
    commentator: Option<bool>,
}

async fn set_workflow_ui(
    State(_st): State<AppState>,
    Path(id): Path<u32>,
    Json(_body): Json<WfBody>,
) -> Json<Value> {
    // TC/commentator not on engine yet — accept and ignore.
    Json(json!({
        "id": id,
        "pair": false,
        "tc": false,
        "commentator": false,
    }))
}

async fn thumb(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Response, UiError> {
    let path = st.hls_dir.join(id.to_string()).join("thumb.jpg");
    if !path.is_file() {
        return Err(UiError::not_found("thumb not found"));
    }
    let bytes = tokio::fs::read(&path).await.map_err(UiError::from)?;
    Ok((
        [(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "no-store")],
        bytes,
    )
        .into_response())
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
