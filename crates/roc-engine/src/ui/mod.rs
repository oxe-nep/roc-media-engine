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

/// Background ticker: honor recording schedules.
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
                if now >= sched.start_at && now < sched.stop_at && !ch.recording {
                    let meta = ui.rec_meta(id);
                    let name = if meta.name.is_empty() {
                        None
                    } else {
                        Some(meta.name.clone())
                    };
                    // Build path under UI recordings root.
                    let path = {
                        let root = ui.recordings_dir();
                        let cat = meta.category.clone();
                        let _ = std::fs::create_dir_all(root.join(&cat));
                        let stamp = now.format("%Y%m%d_%H%M%S");
                        let label = name
                            .clone()
                            .unwrap_or_else(|| format!("ch{id}"))
                            .replace(' ', "_");
                        let ext = orch.recording_ext(id);
                        root.join(&cat)
                            .join(format!("{label}_ch{id}_{stamp}.{ext}"))
                            .to_string_lossy()
                            .into_owned()
                    };
                    if let Err(e) = orch.start_recording(id, Some(path), name, Some(meta.category))
                    {
                        tracing::warn!(channel = id, error = %e, "schedule start failed");
                    } else {
                        ui.mark_recording_started(id);
                        tracing::info!(channel = id, "schedule started recording");
                    }
                } else if now >= sched.stop_at && ch.recording {
                    if let Err(e) = orch.stop_recording(id) {
                        tracing::warn!(channel = id, error = %e, "schedule stop failed");
                    } else {
                        ui.mark_recording_stopped(id);
                        ui.clear_schedule(id);
                        tracing::info!(channel = id, "schedule stopped recording");
                    }
                }
            }
        }
    });
}

/// Fix unused import warning if Response unused in some builds.
#[allow(dead_code)]
fn _resp_type(_: Response) {}
