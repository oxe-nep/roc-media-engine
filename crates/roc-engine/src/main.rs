//! roc-media-engine control plane: HTTP/WS UI API + GStreamer orchestrator.

mod api;
mod orchestrator;
mod ui;
mod workflows;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use tracing_subscriber::EnvFilter;

use roc_config::Config;
use roc_pipelines::create_backend;

use crate::orchestrator::Orchestrator;
use crate::ui::state::UiState;

#[derive(Debug, Parser)]
#[command(name = "roc-media-engine", about = "ROC in-process media engine (GStreamer)")]
struct Args {
    /// Path to YAML config
    #[arg(long, env = "ROC_MEDIA_CONFIG", default_value = "config.yaml")]
    config: PathBuf,

    /// Override bind address (e.g. 0.0.0.0:8090)
    #[arg(long, env = "ROC_MEDIA_BIND")]
    bind: Option<String>,

    /// Write example config.yaml and exit
    #[arg(long)]
    write_example_config: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let args = Args::parse();

    if args.write_example_config {
        let example = Config::example();
        let yaml = serde_yaml::to_string(&example)?;
        std::fs::write(&args.config, yaml)
            .with_context(|| format!("write {}", args.config.display()))?;
        println!("wrote example config to {}", args.config.display());
        return Ok(());
    }

    let mut cfg = if args.config.exists() {
        Config::load(&args.config)?
    } else {
        tracing::warn!(
            path = %args.config.display(),
            "config missing — using built-in example (8 channels)"
        );
        Config::example()
    };

    if let Some(bind) = args.bind {
        cfg.bind = bind;
    }

    if let Ok(hls) = std::env::var("ROC_MEDIA_HLS_DIR") {
        if !hls.trim().is_empty() {
            cfg.hls_dir = PathBuf::from(hls);
        }
    }
    if let Ok(host) = std::env::var("ROC_MEDIA_PUBLIC_HOST") {
        if !host.trim().is_empty() {
            cfg.public_host = host;
        }
    }

    // GST preview writes under ROC_MEDIA_HLS_DIR (see capture.rs).
    std::env::set_var(
        "ROC_MEDIA_HLS_DIR",
        cfg.hls_dir.to_string_lossy().as_ref(),
    );

    std::fs::create_dir_all(&cfg.recordings_dir)?;
    std::fs::create_dir_all(&cfg.preview_dir)?;
    std::fs::create_dir_all(&cfg.hls_dir)?;

    let data_dir = args
        .config
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let backend = create_backend(&cfg);
    let orch = Arc::new(Orchestrator::new(
        cfg.clone(),
        backend,
        args.config.clone(),
    )?);
    let ui = Arc::new(UiState::load(
        &data_dir,
        cfg.public_host.clone(),
        cfg.recordings_dir.clone(),
    ));
    for ch in &cfg.channels {
        ui.ensure_channel(ch.id, &ch.name);
    }
    ui.persist();

    ui::spawn_encode_autostart(orch.clone(), ui.clone());
    ui::spawn_schedule_ticker(orch.clone(), ui.clone());

    let app = ui::router(orch, ui, cfg.hls_dir.clone(), data_dir.clone());
    let addr: SocketAddr = cfg.bind.parse().context("parse bind address")?;
    tracing::info!(%addr, hls = %cfg.hls_dir.display(), "roc-media-engine listening (UI cutover)");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
