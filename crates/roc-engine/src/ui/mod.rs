mod auth;
mod library;
mod playout_media;
mod routes;
mod snapshot;
pub mod state;
mod sysmetrics;

pub use library::safe_file_label;
mod tc;
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

    let mut app = Router::new()
        .merge(ui_routes)
        .merge(native)
        .nest_service("/hls", hls)
        .layer(from_fn(move |req: Request, next: Next| {
            let key = key_for_mw.clone();
            async move { require_api_key(key, req, next).await }
        }));
    if let Some(cors) = cors_from_env() {
        app = app.layer(cors);
    }
    // Log method + path only: the query string can carry `api_key=`.
    app.layer(TraceLayer::new_for_http().make_span_with(|req: &Request| {
        tracing::info_span!("http", method = %req.method(), path = %req.uri().path())
    }))
}

/// True when `API_KEY` is set in the environment.
pub fn api_key_configured() -> bool {
    ApiKey::from_env().is_set()
}

/// CORS is off by default: production is same-origin behind the frontend's
/// nginx. For local dev (`next dev` on :3000 → engine on :8080) set
/// `ROC_MEDIA_CORS_ORIGINS=http://localhost:3000` (comma-separated).
fn cors_from_env() -> Option<CorsLayer> {
    use axum::http::{HeaderName, HeaderValue, Method};
    let raw = std::env::var("ROC_MEDIA_CORS_ORIGINS").ok()?;
    let origins: Vec<HeaderValue> = raw
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .filter_map(|s| HeaderValue::from_str(s).ok())
        .collect();
    if origins.is_empty() {
        return None;
    }
    Some(
        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::OPTIONS])
            .allow_headers([
                axum::http::header::CONTENT_TYPE,
                HeaderName::from_static("x-api-key"),
            ]),
    )
}

/// Background ticker: honor recording schedules for armed PROXY and/or HQ roles.
/// Restore encode capture for channels that were left running (or never marked off).
pub fn spawn_encode_autostart(orch: Arc<Orchestrator>, ui: Arc<UiState>) {
    tokio::spawn(async move {
        // Let HTTP bind and DeckLink settle before hammering starts.
        tokio::time::sleep(Duration::from_secs(2)).await;
        let ids: Vec<u32> = orch.list_channels().into_iter().map(|c| c.id).collect();
        for id in ids {
            // TC workflow owns the DeckLink pair — skip encode restore.
            if ui.workflow_mode(id) == "tc" || ui.tc(id).enabled {
                continue;
            }
            // Explicit false = user stopped; None/true = bring back after restart.
            if ui.encode_wanted(id) == Some(false) {
                continue;
            }
            match orch.channel(id) {
                Ok(ch) if matches!(ch.status, roc_pipelines::ChannelStatus::Running | roc_pipelines::ChannelStatus::Waiting) => {
                    ui.set_encode_wanted(id, true);
                    continue;
                }
                _ => {}
            }
            let o = orch.clone();
            let started = tokio::task::spawn_blocking(move || o.start_capture(id))
                .await
                .unwrap_or_else(|e| Err(anyhow::anyhow!("start_capture panicked: {e}")));
            match started {
                Ok(_) => {
                    ui.set_encode_wanted(id, true);
                    tracing::info!(channel = id, "encode autostart");
                }
                Err(e) => {
                    tracing::warn!(channel = id, error = %e, "encode autostart failed");
                }
            }
            tokio::time::sleep(Duration::from_millis(750)).await;
        }
    });
}

/// Restore TC burn-in loops that were left enabled.
pub fn spawn_tc_autostart(orch: Arc<Orchestrator>, ui: Arc<UiState>, hls_dir: PathBuf) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let hls = hls_dir.to_string_lossy().to_string();
        let ids: Vec<u32> = orch.list_channels().into_iter().map(|c| c.id).collect();
        for id in ids {
            if ui.workflow_mode(id) != "tc" && !ui.tc(id).enabled {
                continue;
            }
            let (o, u, h) = (orch.clone(), ui.clone(), hls.clone());
            let started = tokio::task::spawn_blocking(move || tc::start_tc(o.as_ref(), u.as_ref(), id, &h))
                .await
                .unwrap_or_else(|e| Err(anyhow::anyhow!("start_tc panicked: {e}")));
            match started {
                Ok(_) => tracing::info!(channel = id, "TC burn-in autostart"),
                Err(e) => tracing::warn!(channel = id, error = %e, "TC autostart failed"),
            }
            tokio::time::sleep(Duration::from_millis(900)).await;
        }
    });
}

pub fn spawn_schedule_ticker(orch: Arc<Orchestrator>, ui: Arc<UiState>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let (o, u) = (orch.clone(), ui.clone());
            // Recording start/stop takes the GStreamer op lock — keep it off the reactor.
            if let Err(e) = tokio::task::spawn_blocking(move || schedule_tick(&o, &u)).await {
                tracing::error!(error = %e, "schedule tick panicked");
            }
        }
    });
}

fn schedule_tick(orch: &Arc<Orchestrator>, ui: &Arc<UiState>) {
    {
        let now = Utc::now();
        {
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
                    let label = safe_file_label(
                        &name.clone().unwrap_or_else(|| format!("ch{id}")),
                    );
                    if library::sanitize_segment(&meta.category).is_err() {
                        tracing::warn!(channel = id, category = %meta.category, "schedule skipped: invalid category");
                        continue;
                    }

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
    }
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
    // Callers validate `category` with `library::sanitize_segment` first.
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
