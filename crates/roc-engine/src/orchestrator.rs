use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use parking_lot::Mutex;
use roc_config::{Config, EncodePreset};
use roc_devices::DeviceProbeReport;
use roc_pipelines::{
    ChannelSnapshot, PipelineBackend, PlayoutSnapshot, RecordingRole,
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

fn sanitize_category(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() || raw == "_unsorted" {
        return "_unsorted".into();
    }
    let cleaned: String = raw
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => c,
            ' ' => '_',
            _ => '_',
        })
        .collect();
    let cleaned = cleaned.trim_matches('.').to_string();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "_unsorted".into()
    } else {
        cleaned
    }
}

pub struct Orchestrator {
    pub cfg: Config,
    /// Path to `config.yaml` — presets / channel assignments are written back here.
    config_path: PathBuf,
    backend: Arc<dyn PipelineBackend>,
    /// Runtime encode presets (seeded from YAML; mutable via API).
    presets: Mutex<HashMap<String, EncodePreset>>,
}

impl Orchestrator {
    /// File extension for a REC role. Proxy is always the encoded bitstream in
    /// an MP4 container; HQ follows the channel's record preset (.mp4 / .mxf).
    pub fn recording_ext_for(&self, id: u32, role: RecordingRole) -> &'static str {
        if role == RecordingRole::Proxy {
            return "mp4";
        }
        let preset_id = self
            .backend
            .channel_snapshot(id)
            .ok()
            .map(|c| {
                if c.record_preset.is_empty() {
                    c.encode_preset
                } else {
                    c.record_preset
                }
            })
            .or_else(|| {
                self.cfg.channel(id).ok().map(|c| {
                    self.cfg.record_preset_id_for_channel(c).to_string()
                })
            })
            .unwrap_or_else(|| self.cfg.default_encode_preset.clone());
        let codec = self
            .presets
            .lock()
            .get(&preset_id)
            .map(|p| p.video_codec.clone())
            .unwrap_or_else(|| "nvh264enc".into());
        roc_config::recording_extension(&codec)
    }

    pub fn new(
        cfg: Config,
        backend: Arc<dyn PipelineBackend>,
        config_path: PathBuf,
    ) -> Result<Self> {
        for ch in &cfg.channels {
            let encode = cfg.preset_for_channel(ch)?;
            let record = cfg.record_preset_for_channel(ch)?;
            backend.ensure_channel(ch, encode, record)?;
        }
        let presets = Mutex::new(cfg.encode_presets.clone());
        Ok(Self {
            cfg,
            config_path,
            backend,
            presets,
        })
    }

    /// Persist runtime presets + per-channel proxy/REC selection into config.yaml.
    fn persist_config(&self) {
        let mut cfg = match Config::load(&self.config_path) {
            Ok(c) => c,
            Err(err) => {
                // Fall back to in-memory cfg so we still write something useful.
                tracing::warn!(error = %err, "reload config for persist failed — using memory copy");
                self.cfg.clone()
            }
        };
        cfg.encode_presets = self.presets.lock().clone();
        for snap in self.backend.list_channels() {
            if let Some(ch) = cfg.channels.iter_mut().find(|c| c.id == snap.id) {
                ch.encode_preset = Some(snap.encode_preset.clone());
                ch.record_preset = Some(snap.record_preset.clone());
            }
        }
        if let Err(err) = cfg.save(&self.config_path) {
            tracing::error!(
                path = %self.config_path.display(),
                error = %err,
                "failed to persist config.yaml (presets will be lost on restart)"
            );
        }
    }

    pub fn list_presets(&self) -> Vec<(String, EncodePreset)> {
        let map = self.presets.lock();
        let mut out: Vec<_> = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Upsert a full preset body (Go/FFmpeg field names accepted; normalized to GST).
    pub fn upsert_preset(
        &self,
        id: &str,
        mut preset: EncodePreset,
        create_only: bool,
    ) -> Result<EncodePreset> {
        let id = id.trim();
        if id.is_empty() {
            bail!("preset id is required");
        }
        if preset.label.trim().is_empty() {
            preset.label = id.to_string();
        }
        if preset.video_bitrate.trim().is_empty() {
            bail!("video_bitrate is required");
        }
        preset.normalize_for_gst();
        {
            let mut map = self.presets.lock();
            let exists = map.contains_key(id);
            if create_only && exists {
                bail!("encode preset `{id}` already exists");
            }
            map.insert(id.to_string(), preset.clone());
        }
        // Relunch channels already assigned to this id so bitrate/codec take effect.
        for ch in self.backend.list_channels() {
            if ch.encode_preset == id {
                if let Err(e) = self.backend.apply_encode_preset(ch.id, id, &preset) {
                    tracing::warn!(
                        channel = ch.id,
                        preset_id = id,
                        error = %e,
                        "failed to reapply updated encode preset"
                    );
                }
            }
            if ch.record_preset == id {
                if let Err(e) = self.backend.apply_record_preset(ch.id, id, &preset) {
                    tracing::warn!(
                        channel = ch.id,
                        preset_id = id,
                        error = %e,
                        "failed to reapply updated record preset"
                    );
                }
            }
        }
        self.persist_config();
        Ok(preset)
    }

    pub fn delete_preset(&self, id: &str) -> Result<()> {
        let id = id.trim();
        if id.is_empty() {
            bail!("preset id is required");
        }
        {
            let mut map = self.presets.lock();
            if !map.contains_key(id) {
                bail!("encode preset `{id}` not found");
            }
            if map.len() <= 1 {
                bail!("cannot delete the last encode preset");
            }
            map.remove(id);
        }
        self.persist_config();
        Ok(())
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

    /// File name (without directory) for an auto-named recording. The proxy gets a
    /// `_proxy` suffix so it never collides with an encoded HQ `.mp4` started in
    /// the same second.
    pub fn recording_file_name(
        &self,
        id: u32,
        role: RecordingRole,
        label: &str,
        stamp: &str,
    ) -> String {
        let suffix = match role {
            RecordingRole::Proxy => "_proxy",
            RecordingRole::Hq => "",
        };
        let ext = self.recording_ext_for(id, role);
        format!("{label}_ch{id}_{stamp}{suffix}.{ext}")
    }

    /// Start HQ recording (backward-compatible entry point for `start_recording`).
    pub fn start_recording(
        &self,
        id: u32,
        path: Option<String>,
        label: Option<String>,
        category: Option<String>,
    ) -> Result<ChannelSnapshot> {
        self.start_hq_recording(id, path, label, category)
    }

    pub fn start_proxy_recording(
        &self,
        id: u32,
        path: Option<String>,
        label: Option<String>,
        category: Option<String>,
    ) -> Result<ChannelSnapshot> {
        self.start_role_recording(id, RecordingRole::Proxy, path, label, category)
    }

    pub fn start_hq_recording(
        &self,
        id: u32,
        path: Option<String>,
        label: Option<String>,
        category: Option<String>,
    ) -> Result<ChannelSnapshot> {
        self.start_role_recording(id, RecordingRole::Hq, path, label, category)
    }

    fn start_role_recording(
        &self,
        id: u32,
        role: RecordingRole,
        path: Option<String>,
        label: Option<String>,
        category: Option<String>,
    ) -> Result<ChannelSnapshot> {
        let path_str = if let Some(p) = path.filter(|s| !s.trim().is_empty()) {
            let pb = PathBuf::from(&p);
            if let Some(parent) = pb.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create recording parent {}", parent.display()))?;
            }
            p
        } else {
            // Fallback when called without Go/UI path (soak scripts / direct API).
            let ch = self.cfg.channel(id)?;
            let name = label.unwrap_or_else(|| ch.name.replace(' ', "_"));
            let stamp = stamp_now();
            let cat = sanitize_category(category.as_deref().unwrap_or("_unsorted"));
            let dir = self.cfg.recordings_dir.join(&cat);
            std::fs::create_dir_all(&dir)?;
            dir.join(self.recording_file_name(id, role, &name, &stamp))
                .to_string_lossy()
                .into_owned()
        };
        match role {
            RecordingRole::Proxy => self.backend.start_proxy_recording(id, &path_str)?,
            RecordingRole::Hq => self.backend.start_hq_recording(id, &path_str)?,
        }
        self.backend.channel_snapshot(id)
    }

    /// Stop HQ recording (backward-compatible entry point for `stop_recording`).
    pub fn stop_recording(&self, id: u32) -> Result<ChannelSnapshot> {
        self.stop_hq_recording(id)
    }

    pub fn stop_proxy_recording(&self, id: u32) -> Result<ChannelSnapshot> {
        self.backend.stop_proxy_recording(id)?;
        self.backend.channel_snapshot(id)
    }

    pub fn stop_hq_recording(&self, id: u32) -> Result<ChannelSnapshot> {
        self.backend.stop_hq_recording(id)?;
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

    pub fn start_webrtc_preview(
        &self,
        id: u32,
        pair: u8,
        signal_tx: roc_pipelines::PreviewSignalTx,
    ) -> Result<String> {
        self.backend.start_webrtc_preview(id, pair, signal_tx)
    }

    pub fn set_webrtc_answer(&self, id: u32, sdp: &str) -> Result<()> {
        self.backend.set_webrtc_answer(id, sdp)
    }

    pub fn add_webrtc_ice(&self, id: u32, sdp_mline_index: u32, candidate: &str) -> Result<()> {
        self.backend.add_webrtc_ice(id, sdp_mline_index, candidate)
    }

    pub fn stop_webrtc_preview_session(&self, id: u32, session_id: &str) -> Result<()> {
        self.backend.stop_webrtc_preview_session(id, session_id)
    }

    pub fn set_encode_preset(&self, id: u32, preset_id: &str) -> Result<ChannelSnapshot> {
        let preset = self
            .presets
            .lock()
            .get(preset_id)
            .cloned()
            .with_context(|| format!("encode preset `{preset_id}` not found"))?;
        self.backend
            .apply_encode_preset(id, preset_id, &preset)?;
        self.persist_config();
        self.backend.channel_snapshot(id)
    }

    pub fn set_record_preset(&self, id: u32, preset_id: &str) -> Result<ChannelSnapshot> {
        let preset = self
            .presets
            .lock()
            .get(preset_id)
            .cloned()
            .with_context(|| format!("record preset `{preset_id}` not found"))?;
        self.backend
            .apply_record_preset(id, preset_id, &preset)?;
        self.persist_config();
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
                    format_code: c.format_code.clone(),
                    last_error: None,
                    audio_peaks: None,
                });
            }
        }
        live.sort_by(|a, b| a.id.cmp(&b.id));
        live
    }

    pub fn start_playout(
        &self,
        client_id: &str,
        source: String,
        format_code: Option<String>,
    ) -> Result<PlayoutSnapshot> {
        let client = self
            .cfg
            .playout
            .iter()
            .find(|c| c.id == client_id)
            .with_context(|| format!("playout client {client_id} not in config"))?;
        let mut client = client.clone();
        if let Some(fc) = format_code.filter(|s| !s.trim().is_empty()) {
            client.format_code = Some(fc);
        }
        self.backend.start_playout(&client, &source)?;
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

    pub fn pause_playout(&self, client_id: &str) -> Result<PlayoutSnapshot> {
        self.backend.pause_playout(client_id)?;
        self.list_playout()
            .into_iter()
            .find(|p| p.id == client_id)
            .context("playout snapshot missing after pause")
    }

    pub fn resume_playout(&self, client_id: &str) -> Result<PlayoutSnapshot> {
        self.backend.resume_playout(client_id)?;
        self.list_playout()
            .into_iter()
            .find(|p| p.id == client_id)
            .context("playout snapshot missing after resume")
    }
}
