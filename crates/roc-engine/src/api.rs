use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
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
}

async fn start_record(
    State(orch): State<ApiState>,
    Path(id): Path<u32>,
    Query(q): Query<RecordQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = orch.start_recording(id, q.label).map_err(ApiError::from)?;
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

async fn list_playout(State(orch): State<ApiState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "playout": orch.list_playout() }))
}

#[derive(Debug, Deserialize)]
pub struct PlayoutBody {
    pub source: String,
}

async fn start_playout(
    State(orch): State<ApiState>,
    Path(id): Path<String>,
    Json(body): Json<PlayoutBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = orch
        .start_playout(&id, body.source)
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

impl From<anyhow::Error> for ApiError {
    fn from(value: anyhow::Error) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: format!("{value:#}"),
        }
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
