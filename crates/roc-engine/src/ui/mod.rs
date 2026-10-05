mod auth;
mod library;
mod playout_media;
mod routes;
mod snapshot;
pub mod state;
mod sysmetrics;
mod ws;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::Request;
use axum::middleware::{from_fn, Next};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use chrono::Utc;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::orchestrator::Orchestrator;
use crate::ui::auth::{require_api_key, ApiKey};
use crate::ui::playout_media::MediaStore;
use crate::ui::state::UiState;
use crate::ui::sysmetrics::Collector as SysCollector;
use roc_pipelines::RecordingRole;

#[derive(Clone)]
pub struct AppState {
    pub orch: Arc<Orchestrator>,
    pub ui: Arc<UiState>,
    pub hls_dir: PathBuf,
    pub sys: Arc<SysCollector>,
    pub playout_media: Arc<MediaStore>,
}

pub fn router(
    orch: Arc<Orchestrator>,
    ui: Arc<UiState>,
    hls_dir: PathBuf,
    data_dir: PathBuf,
) -> Router {
    let _ = std::fs::create_dir_all(&hls_dir);
    let sys = Arc::new(SysCollector::new(ui.recordings_dir()));
    let playout_media = Arc::new(MediaStore::open(&data_dir));
    let state = AppState {
        orch: orch.clone(),
        ui: ui.clone(),
        hls_dir: hls_dir.clone(),
        sys,
        playout_media,
    };

    let api_key = ApiKey::from_env();
    let key_for_mw = api_key.clone();

    let hls = ServeDir::new(hls_dir.clone()).append_index_html_on_directories(false);

    let ui_routes = routes::router()
        .route("/ws", get(ws::ws_handler))
        .with_state(state.clone());

    // Native engine API (kept for soak/scripts) + UI façade + static HLS.
    let native = crate::api::router(orch);

    Router::new()
        .merge(ui_routes)
        .merge(native)
        .nest_service("/hls", hls)
        .layer(from_fn(move |req: Request, next: Next| {
            let key = key_for_mw.clone();
            async move { require_api_key(key, req, next).await }
        }))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}

/// Background ticker: honor recording schedules for armed PROXY and/or HQ roles.
pub fn spawn_schedule_ticker(orch: Arc<Orchestrator>, ui: Arc<UiState>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tick.tick().await;
            let now = Utc::now();
            for (id, sched) in ui.all_schedules() {
                let ch = match orch.channel(id) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                if now >= sched.start_at && now < sched.stop_at {
                    let meta = ui.rec_meta(id);
                    let name = if meta.name.is_empty() {
                        None
                    } else {
                        Some(meta.name.clone())
                    };
                    let stamp = now.format("%Y%m%d_%H%M%S").to_string();
                    let label = name
                        .clone()
                        .unwrap_or_else(|| format!("ch{id}"))
                        .replace(' ', "_");

                    if sched.arm_hq && !ch.hq_recording {
                        let path = schedule_recording_path(
                            &ui,
                            &orch,
                            id,
                            RecordingRole::Hq,
                            &meta.category,
                            &label,
                            &stamp,
                        );
                        match orch.start_hq_recording(
                            id,
                            Some(path),
                            name.clone(),
                            Some(meta.category.clone()),
                        ) {
                            Ok(_) => {
                                ui.mark_recording_started_role(id, RecordingRole::Hq);
                                tracing::info!(channel = id, "schedule started HQ recording");
                            }
                            Err(e) => {
                                tracing::warn!(channel = id, error = %e, "schedule HQ start failed");
                            }
                        }
                    }

                    if sched.arm_proxy && !ch.proxy_recording {
                        let path = schedule_recording_path(
                            &ui,
                            &orch,
                            id,
                            RecordingRole::Proxy,
                            &meta.category,
                            &label,
                            &stamp,
                        );
                        match orch.start_proxy_recording(
                            id,
                            Some(path),
                            name.clone(),
                            Some(meta.category.clone()),
                        ) {
                            Ok(_) => {
                                ui.mark_recording_started_role(id, RecordingRole::Proxy);
                                tracing::info!(channel = id, "schedule started proxy recording");
                            }
                            Err(e) => {
                                tracing::warn!(
                                    channel = id,
                                    error = %e,
                                    "schedule proxy start failed"
                                );
                            }
                        }
                    }
                } else if now >= sched.stop_at {
                    let mut still_recording = false;

                    if sched.arm_hq && ch.hq_recording {
                        match orch.stop_hq_recording(id) {
                            Ok(_) => {
                                ui.mark_recording_stopped_role(id, RecordingRole::Hq);
                                tracing::info!(channel = id, "schedule stopped HQ recording");
                            }
                            Err(e) => {
                                tracing::warn!(channel = id, error = %e, "schedule HQ stop failed");
                                still_recording = true;
                            }
                        }
                    }

                    if sched.arm_proxy && ch.proxy_recording {
                        match orch.stop_proxy_recording(id) {
                            Ok(_) => {
                                ui.mark_recording_stopped_role(id, RecordingRole::Proxy);
                                tracing::info!(channel = id, "schedule stopped proxy recording");
                            }
                            Err(e) => {
                                tracing::warn!(
                                    channel = id,
                                    error = %e,
                                    "schedule proxy stop failed"
                                );
                                still_recording = true;
                            }
                        }
                    }

                    if !still_recording {
                        ui.clear_schedule(id);
                        tracing::info!(channel = id, "schedule cleared");
                    }
                }
            }
        }
    });
}

fn schedule_recording_path(
    ui: &UiState,
    orch: &Orchestrator,
    id: u32,
    role: RecordingRole,
    category: &str,
    label: &str,
    stamp: &str,
) -> String {
    let root = ui.recordings_dir();
    let _ = std::fs::create_dir_all(root.join(category));
    root.join(category)
        .join(orch.recording_file_name(id, role, label, stamp))
        .to_string_lossy()
        .into_owned()
}

/// Fix unused import warning if Response unused in some builds.
#[allow(dead_code)]
fn _resp_type(_: Response) {}
