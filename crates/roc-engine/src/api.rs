use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use roc_pipelines::WorkflowKind;

use crate::orchestrator::Orchestrator;
use crate::workflows;

pub fn router(orch: Arc<Orchestrator>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/health", get(health))
        .route("/api/devices", get(devices))
        .route("/api/channels", get(list_channels))
        .route("/api/channels/{id}", get(get_channel))
        .route("/api/channels/{id}/start", post(start_capture))
        .route("/api/channels/{id}/stop", post(stop_capture))
        .route("/api/channels/{id}/record/start", post(start_record))
        .route("/api/channels/{id}/record/stop", post(stop_record))
        .route("/api/channels/{id}/srt/start", post(start_srt))
        .route("/api/channels/{id}/srt/stop", post(stop_srt))
        .route("/api/channels/{id}/encode-preset", post(set_encode_preset))
        .route("/api/encode/presets", get(list_encode_presets).post(create_encode_preset))
        .route(
            "/api/encode/presets/{id}",
            put(upsert_encode_preset).delete(delete_encode_preset),
        )
        .route("/api/meters", get(meters))
        .route("/api/playout", get(list_playout))
        .route("/api/playout/{id}/start", post(start_playout))
        .route("/api/playout/{id}/stop", post(stop_playout))
        .route("/api/workflows", get(list_workflows))
        .route("/api/workflows/{channel_id}", post(set_workflow))
        .route("/api/adapter/manifest", get(adapter_manifest))
        .with_state(orch)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}

type ApiState = Arc<Orchestrator>;

async fn health(State(orch): State<ApiState>) -> Json<serde_json::Value> {
    Json(orch.health())
}

async fn devices(State(orch): State<ApiState>) -> Result<Json<serde_json::Value>, ApiError> {
    let report = orch.probe().map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(report).unwrap()))
}

async fn list_channels(State(orch): State<ApiState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "channels": orch.list_channels() }))
}

/// Peak meters for all channels (`level` element → dBFS). Polled by Go WS at ~80ms.
async fn meters(State(orch): State<ApiState>) -> Json<serde_json::Value> {
    let mut map = serde_json::Map::new();
    for ch in orch.list_channels() {
        let peaks = ch.audio_peaks.unwrap_or_else(|| vec![-90.0; 8]);
        let l = peaks.first().copied().unwrap_or(-90.0);
        let r = peaks.get(1).copied().unwrap_or(-90.0);
        map.insert(
            ch.id.to_string(),
            serde_json::json!({ "l": l, "r": r, "channels": peaks }),
        );
    }
    Json(serde_json::json!({ "meters": map }))
}

async fn get_channel(
    State(orch): State<ApiState>,
    Path(id): Path<u32>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = orch.channel(id).map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(snap).unwrap()))
}

async fn start_capture(
    State(orch): State<ApiState>,
    Path(id): Path<u32>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = orch.start_capture(id).map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(snap).unwrap()))
}

async fn stop_capture(
    State(orch): State<ApiState>,
    Path(id): Path<u32>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = orch.stop_capture(id).map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(snap).unwrap()))
}

#[derive(Debug, Deserialize)]
pub struct RecordQuery {
    pub label: Option<String>,
    pub category: Option<String>,
    /// Absolute path from Go/UI library root. When set, overrides label/category layout.
    pub path: Option<String>,
}

async fn start_record(
    State(orch): State<ApiState>,
    Path(id): Path<u32>,
    Query(q): Query<RecordQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = orch
        .start_recording(id, q.path, q.label, q.category)
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(snap).unwrap()))
}

async fn stop_record(
    State(orch): State<ApiState>,
    Path(id): Path<u32>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = orch.stop_recording(id).map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(snap).unwrap()))
}

#[derive(Debug, Deserialize)]
pub struct SrtBody {
    pub url: Option<String>,
}

async fn start_srt(
    State(orch): State<ApiState>,
    Path(id): Path<u32>,
    body: Result<Json<SrtBody>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let url = body.ok().and_then(|b| b.url.clone());
    let snap = orch.start_srt(id, url).map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(snap).unwrap()))
}

async fn stop_srt(
    State(orch): State<ApiState>,
    Path(id): Path<u32>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = orch.stop_srt(id).map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(snap).unwrap()))
}

#[derive(Debug, Deserialize)]
pub struct EncodePresetBody {
    pub preset: String,
}

async fn set_encode_preset(
    State(orch): State<ApiState>,
    Path(id): Path<u32>,
    Json(body): Json<EncodePresetBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.preset.trim().is_empty() {
        return Err(ApiError::bad_request("preset is required"));
    }
    let snap = orch
        .set_encode_preset(id, body.preset.trim())
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(snap).unwrap()))
}

/// Full preset body (Go UI / FFmpeg field names accepted).
#[derive(Debug, Deserialize)]
pub struct EncodePresetDefBody {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub video_codec: Option<String>,
    pub video_bitrate: String,
    #[serde(default)]
    pub video_maxrate: Option<String>,
    #[serde(default)]
    pub video_bufsize: Option<String>,
    #[serde(default)]
    pub video_preset: Option<String>,
    #[serde(default)]
    pub video_gop: Option<u32>,
    #[serde(default)]
    pub audio_bitrate: Option<String>,
    #[serde(default)]
    pub audio_channels: Option<u32>,
}

fn preset_from_body(id: &str, body: EncodePresetDefBody) -> roc_config::EncodePreset {
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

fn preset_json(id: &str, p: &roc_config::EncodePreset) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "label": p.label,
        "video_codec": p.video_codec,
        "video_bitrate": p.video_bitrate,
        "video_maxrate": p.video_maxrate,
        "video_bufsize": p.video_bufsize,
        "video_preset": p.video_preset,
        "video_gop": p.video_gop,
        "audio_bitrate": p.audio_bitrate,
        "audio_channels": p.audio_channels,
    })
}

async fn list_encode_presets(State(orch): State<ApiState>) -> Json<serde_json::Value> {
    let presets: Vec<_> = orch
        .list_presets()
        .iter()
        .map(|(id, p)| preset_json(id, p))
        .collect();
    Json(serde_json::json!({ "presets": presets }))
}

async fn create_encode_preset(
    State(orch): State<ApiState>,
    Json(body): Json<EncodePresetDefBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = body
        .id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("id is required"))?
        .to_string();
    let preset = preset_from_body(&id, body);
    let saved = orch
        .upsert_preset(&id, preset, true)
        .map_err(ApiError::from)?;
    Ok(Json(preset_json(&id, &saved)))
}

async fn upsert_encode_preset(
    State(orch): State<ApiState>,
    Path(id): Path<String>,
    Json(body): Json<EncodePresetDefBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if id.trim().is_empty() {
        return Err(ApiError::bad_request("preset id is required"));
    }
    let preset = preset_from_body(&id, body);
    let saved = orch
        .upsert_preset(&id, preset, false)
        .map_err(ApiError::from)?;
    Ok(Json(preset_json(&id, &saved)))
}

async fn delete_encode_preset(
    State(orch): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    orch.delete_preset(&id).map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({ "status": "deleted", "id": id })))
}

async fn list_playout(State(orch): State<ApiState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "playout": orch.list_playout() }))
}

#[derive(Debug, Deserialize)]
pub struct PlayoutBody {
    pub source: String,
    /// Optional BMD/UI format code (e.g. Hp50, Hi50). Overrides config when set.
    #[serde(default)]
    pub format_code: Option<String>,
}

async fn start_playout(
    State(orch): State<ApiState>,
    Path(id): Path<String>,
    Json(body): Json<PlayoutBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = orch
        .start_playout(&id, body.source, body.format_code)
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(snap).unwrap()))
}

async fn stop_playout(
    State(orch): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = orch.stop_playout(&id).map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(snap).unwrap()))
}

async fn list_workflows(State(orch): State<ApiState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "workflows": orch.list_workflows() }))
}

#[derive(Debug, Deserialize)]
pub struct WorkflowBody {
    pub kind: String,
    pub active: bool,
}

async fn set_workflow(
    State(orch): State<ApiState>,
    Path(channel_id): Path<u32>,
    Json(body): Json<WorkflowBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let kind = match body.kind.as_str() {
        "pair" => WorkflowKind::Pair,
        "tc" | "timecode" => WorkflowKind::Timecode,
        "commentator" => WorkflowKind::Commentator,
        other => {
            return Err(ApiError {
                status: StatusCode::BAD_REQUEST,
                message: format!("unknown workflow kind: {other}"),
            })
        }
    };
    orch.set_workflow(channel_id, kind, body.active)
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({
        "channel_id": channel_id,
        "workflows": orch.list_workflows(),
    })))
}

async fn adapter_manifest() -> Json<serde_json::Value> {
    Json(workflows::adapter_manifest())
}

pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(value: anyhow::Error) -> Self {
        let message = format!("{value:#}");
        let status = if message.contains("not found") {
            StatusCode::NOT_FOUND
        } else if message.contains("stop recording")
            || message.contains("already exists")
            || message.contains("cannot delete the last")
        {
            StatusCode::CONFLICT
        } else {
            StatusCode::BAD_REQUEST
        };
        Self { status, message }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}
