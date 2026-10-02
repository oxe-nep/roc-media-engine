use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use roc_config::Config;
use roc_devices::DeviceProbeReport;
use roc_pipelines::{
    ChannelSnapshot, PipelineBackend, PlayoutSnapshot, WorkflowKind, WorkflowSnapshot,
};

fn stamp_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86400;
    let tod = secs % 86400;
    let h = tod / 3600;
    let m = (tod % 3600) / 60;
    let s = tod % 60;
    format!("epoch{days}_{h:02}{m:02}{s:02}")
}

pub struct Orchestrator {
    pub cfg: Config,
    backend: Arc<dyn PipelineBackend>,
}

impl Orchestrator {
    pub fn new(cfg: Config, backend: Arc<dyn PipelineBackend>) -> Result<Self> {
        for ch in &cfg.channels {
            let preset = cfg.preset_for_channel(ch)?;
            backend.ensure_channel(ch, preset)?;
        }
        Ok(Self { cfg, backend })
    }

    pub fn health(&self) -> serde_json::Value {
        serde_json::json!({
            "ok": true,
            "service": "roc-media-engine",
            "nvenc_used": self.backend.nvenc_used(),
            "nvenc_limit": self.backend.nvenc_limit(),
            "channels": self.backend.list_channels().len(),
        })
    }

    pub fn probe(&self) -> Result<DeviceProbeReport> {
        self.backend.probe_devices()
    }

    pub fn list_channels(&self) -> Vec<ChannelSnapshot> {
        self.backend.list_channels()
    }

    pub fn channel(&self, id: u32) -> Result<ChannelSnapshot> {
        self.backend.channel_snapshot(id)
    }

    pub fn start_capture(&self, id: u32) -> Result<ChannelSnapshot> {
        self.backend.start_capture(id)?;
        self.backend.channel_snapshot(id)
    }

    pub fn stop_capture(&self, id: u32) -> Result<ChannelSnapshot> {
        self.backend.stop_capture(id)?;
        self.backend.channel_snapshot(id)
    }

    pub fn start_recording(&self, id: u32, label: Option<String>) -> Result<ChannelSnapshot> {
        let ch = self.cfg.channel(id)?;
        let name = label.unwrap_or_else(|| ch.name.replace(' ', "_"));
        let stamp = stamp_now();
        let dir = self.cfg.recordings_dir.join("_unsorted");
        std::fs::create_dir_all(&dir)?;
        let path: PathBuf = dir.join(format!("{name}_{stamp}.mp4"));
        let path_str = path.to_string_lossy().to_string();
        self.backend.start_recording(id, &path_str)?;
        self.backend.channel_snapshot(id)
    }

    pub fn stop_recording(&self, id: u32) -> Result<ChannelSnapshot> {
        self.backend.stop_recording(id)?;
        self.backend.channel_snapshot(id)
    }

    pub fn start_srt(&self, id: u32, url: Option<String>) -> Result<ChannelSnapshot> {
        let ch = self.cfg.channel(id)?;
        let url = url
            .or_else(|| ch.srt_url.clone())
            .with_context(|| format!("no SRT URL for channel {id}"))?;
        self.backend.start_srt(id, &url)?;
        self.backend.channel_snapshot(id)
    }

    pub fn stop_srt(&self, id: u32) -> Result<ChannelSnapshot> {
        self.backend.stop_srt(id)?;
        self.backend.channel_snapshot(id)
    }

    pub fn list_playout(&self) -> Vec<PlayoutSnapshot> {
        let mut live = self.backend.list_playout();
        for c in &self.cfg.playout {
            if !live.iter().any(|p| p.id == c.id) {
                live.push(PlayoutSnapshot {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    status: roc_pipelines::ChannelStatus::Stopped,
                    device: c.device.clone(),
                    source: None,
                    last_error: None,
                });
            }
        }
        live.sort_by(|a, b| a.id.cmp(&b.id));
        live
    }

    pub fn start_playout(&self, client_id: &str, source: String) -> Result<PlayoutSnapshot> {
        let client = self
            .cfg
            .playout
            .iter()
            .find(|c| c.id == client_id)
            .with_context(|| format!("playout client {client_id} not in config"))?;
        self.backend.start_playout(client, &source)?;
        self.list_playout()
            .into_iter()
            .find(|p| p.id == client_id)
            .context("playout snapshot missing after start")
    }

    pub fn stop_playout(&self, client_id: &str) -> Result<PlayoutSnapshot> {
        self.backend.stop_playout(client_id)?;
        self.list_playout()
            .into_iter()
            .find(|p| p.id == client_id)
            .context("playout snapshot missing after stop")
    }

    pub fn set_workflow(&self, channel_id: u32, kind: WorkflowKind, active: bool) -> Result<()> {
        self.backend.set_workflow(channel_id, kind, active)
    }

    pub fn list_workflows(&self) -> Vec<WorkflowSnapshot> {
        self.backend.list_workflows()
    }
}
