//! roc-media-engine control plane: HTTP API + channel orchestrator.

mod api;
mod orchestrator;
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
        let yaml = serde_yaml_string(&example)?;
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

    std::fs::create_dir_all(&cfg.recordings_dir)?;
    std::fs::create_dir_all(&cfg.preview_dir)?;

    let backend = create_backend(&cfg);
    let orch = Arc::new(Orchestrator::new(cfg.clone(), backend)?);

    let app = api::router(orch.clone());
    let addr: SocketAddr = cfg.bind.parse().context("parse bind address")?;
    tracing::info!(%addr, "roc-media-engine listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn serde_yaml_string(cfg: &Config) -> Result<String> {
    // serde_yaml is on roc-config; re-serialize via JSON→YAML-ish by using debug isn't ideal.
    // Engine depends on roc-config only — add serde_yaml here.
    Ok(serde_yaml::to_string(cfg)?)
}
