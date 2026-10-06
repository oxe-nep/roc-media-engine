use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::gst_task::run_blocking;
use crate::orchestrator::Orchestrator;
use crate::workflows;

/// Native engine control routes (`/api/channels…`). UI-compatible façades live in `ui::`.
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
        .route("/api/channels/{id}/record/proxy/start", post(start_record_proxy))
        .route("/api/channels/{id}/record/proxy/stop", post(stop_record_proxy))
        .route("/api/channels/{id}/record/hq/start", post(start_record_hq))
        .route("/api/channels/{id}/record/hq/stop", post(stop_record_hq))
        .route("/api/channels/{id}/srt/start", post(start_srt))
        .route("/api/channels/{id}/srt/stop", post(stop_srt))
        .route("/api/channels/{id}/encode-preset", post(set_encode_preset))
        .route("/api/channels/{id}/record-preset", post(set_record_preset))
        .route("/api/meters", get(meters))
        .route("/api/adapter/manifest", get(adapter_manifest))
        .with_state(orch)
}

type ApiState = Arc<Orchestrator>;

async fn health(State(orch): State<ApiState>) -> Json<serde_json::Value> {
    Json(orch.health())
}

async fn devices(State(orch): State<ApiState>) -> Result<Json<serde_json::Value>, ApiError> {
    let report = run_blocking(move || orch.probe())
        .await
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(report).unwrap()))
}

async fn list_channels(State(orch): State<ApiState>) -> Json<serde_json::Value> {
    let channels = run_blocking(move || orch.list_channels()).await;
    Json(serde_json::json!({ "channels": channels }))
}

/// Peak meters for all channels (`level` element → dBFS).
async fn meters(State(orch): State<ApiState>) -> Json<serde_json::Value> {
    let channels = run_blocking(move || orch.list_channel_meters()).await;
    let mut map = serde_json::Map::new();
    for ch in channels {
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
    let snap = run_blocking(move || orch.channel(id))
        .await
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(snap).unwrap()))
}

async fn start_capture(
    State(orch): State<ApiState>,
    Path(id): Path<u32>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = run_blocking(move || orch.start_capture(id))
        .await
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(snap).unwrap()))
}

async fn stop_capture(
    State(orch): State<ApiState>,
    Path(id): Path<u32>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = run_blocking(move || orch.stop_capture(id))
        .await
        .map_err(ApiError::from)?;
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
    let snap = run_blocking(move || {
        orch.start_recording(id, q.path, q.label, q.category)
    })
    .await
    .map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(snap).unwrap()))
}

async fn stop_record(
    State(orch): State<ApiState>,
    Path(id): Path<u32>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = run_blocking(move || orch.stop_recording(id))
        .await
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(snap).unwrap()))
}

async fn start_record_proxy(
    State(orch): State<ApiState>,
    Path(id): Path<u32>,
    Query(q): Query<RecordQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = run_blocking(move || {
        orch.start_proxy_recording(id, q.path, q.label, q.category)
    })
    .await
    .map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(snap).unwrap()))
}

async fn stop_record_proxy(
    State(orch): State<ApiState>,
    Path(id): Path<u32>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = run_blocking(move || orch.stop_proxy_recording(id))
        .await
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(snap).unwrap()))
}

async fn start_record_hq(
    State(orch): State<ApiState>,
    Path(id): Path<u32>,
    Query(q): Query<RecordQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = run_blocking(move || {
        orch.start_hq_recording(id, q.path, q.label, q.category)
    })
    .await
    .map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(snap).unwrap()))
}

async fn stop_record_hq(
    State(orch): State<ApiState>,
    Path(id): Path<u32>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = run_blocking(move || orch.stop_hq_recording(id))
        .await
        .map_err(ApiError::from)?;
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
    let snap = run_blocking(move || orch.start_srt(id, url))
        .await
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(snap).unwrap()))
}

async fn stop_srt(
    State(orch): State<ApiState>,
    Path(id): Path<u32>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = run_blocking(move || orch.stop_srt(id))
        .await
        .map_err(ApiError::from)?;
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
    let preset = body.preset.trim().to_string();
    let snap = run_blocking(move || orch.set_encode_preset(id, &preset))
        .await
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(snap).unwrap()))
}

async fn set_record_preset(
    State(orch): State<ApiState>,
    Path(id): Path<u32>,
    Json(body): Json<EncodePresetBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.preset.trim().is_empty() {
        return Err(ApiError::bad_request("preset is required"));
    }
    let preset = body.preset.trim().to_string();
    let snap = run_blocking(move || orch.set_record_preset(id, &preset))
        .await
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(snap).unwrap()))
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
