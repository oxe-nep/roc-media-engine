//! In-process mock backend for development without GStreamer/DeckLink.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use parking_lot::Mutex;
use roc_config::{ChannelConfig, EncodePreset, PlayoutClientConfig};
use roc_devices::DeviceProbeReport;

use crate::{
    ChannelSnapshot, ChannelStatus, PipelineBackend, PlayoutSnapshot, RecordingRole,
    WorkflowKind, WorkflowSnapshot,
};

struct Chan {
    name: String,
    encode_preset: String,
    record_preset: String,
    status: ChannelStatus,
    proxy_recording: bool,
    hq_recording: bool,
    srt: bool,
    proxy_recording_path: Option<String>,
    hq_recording_path: Option<String>,
    srt_url: Option<String>,
    last_error: Option<String>,
    udp_egress: Option<String>,
    started_at: Option<Instant>,
}

struct Play {
    name: String,
    device: String,
    status: ChannelStatus,
    source: Option<String>,
    last_error: Option<String>,
}

pub struct MockBackend {
    max_nvenc: usize,
    nvenc_used: AtomicUsize,
    channels: Mutex<HashMap<u32, Chan>>,
    playout: Mutex<HashMap<String, Play>>,
    workflows: Mutex<HashMap<u32, (WorkflowKind, bool)>>,
}

impl MockBackend {
    pub fn new(max_nvenc: usize) -> Self {
        Self {
            max_nvenc,
            nvenc_used: AtomicUsize::new(0),
            channels: Mutex::new(HashMap::new()),
            playout: Mutex::new(HashMap::new()),
            workflows: Mutex::new(HashMap::new()),
        }
    }

    fn bump_running_status(ch: &mut Chan) {
        if let Some(t0) = ch.started_at {
            if t0.elapsed() > Duration::from_millis(200) && ch.status == ChannelStatus::Waiting {
                ch.status = ChannelStatus::Running;
            }
        }
    }

    fn start_role_recording(&self, channel_id: u32, role: RecordingRole, path: &str) -> Result<()> {
        let mut map = self.channels.lock();
        let ch = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        Self::bump_running_status(ch);
        if ch.status != ChannelStatus::Running && ch.status != ChannelStatus::Waiting {
            bail!("capture not running on channel {channel_id}");
        }
        match role {
            RecordingRole::Proxy => {
                if ch.proxy_recording {
                    bail!("already recording proxy");
                }
                ch.proxy_recording = true;
                ch.proxy_recording_path = Some(path.to_string());
            }
            RecordingRole::Hq => {
                if ch.hq_recording {
                    bail!("already recording hq");
                }
                ch.hq_recording = true;
                ch.hq_recording_path = Some(path.to_string());
            }
        }
        Ok(())
    }

    fn stop_role_recording(&self, channel_id: u32, role: RecordingRole) -> Result<()> {
        let mut map = self.channels.lock();
        let ch = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        match role {
            RecordingRole::Proxy => {
                ch.proxy_recording = false;
                ch.proxy_recording_path = None;
            }
            RecordingRole::Hq => {
                ch.hq_recording = false;
                ch.hq_recording_path = None;
            }
        }
        Ok(())
    }

    fn snapshot_of(&self, id: u32, ch: &Chan) -> ChannelSnapshot {
        ChannelSnapshot {
            id,
            name: ch.name.clone(),
            status: ch.status,
            encode_preset: ch.encode_preset.clone(),
            record_preset: ch.record_preset.clone(),
            video_bitrate_kbps: if matches!(ch.status, ChannelStatus::Running) {
                Some(11_500.0)
            } else {
                None
            },
            srt_bitrate_kbps: if ch.srt { Some(11_800.0) } else { None },
            recording: ch.proxy_recording || ch.hq_recording,
            proxy_recording: ch.proxy_recording,
            hq_recording: ch.hq_recording,
            srt: ch.srt,
            recording_path: ch
                .hq_recording_path
                .clone()
                .or_else(|| ch.proxy_recording_path.clone()),
            proxy_recording_path: ch.proxy_recording_path.clone(),
            hq_recording_path: ch.hq_recording_path.clone(),
            srt_url: ch.srt_url.clone(),
            last_error: ch.last_error.clone(),
            nvenc_slots_used: self.nvenc_used.load(Ordering::SeqCst),
            configured_mode: "auto".into(),
            locked_mode: Some("mock".into()),
            input_format: Some("1920x1080p50/1 (mock)".into()),
            audio_peaks: Some(vec![-90.0; 8]),
            preview_epoch: 0,
        }
    }
}

impl PipelineBackend for MockBackend {
    fn probe_devices(&self) -> Result<DeviceProbeReport> {
        Ok(DeviceProbeReport::empty_mock())
    }

    fn ensure_channel(
        &self,
        ch: &ChannelConfig,
        encode: &EncodePreset,
        record: &EncodePreset,
    ) -> Result<()> {
        let encode_id = ch
            .encode_preset
            .clone()
            .unwrap_or_else(|| "hq".into());
        let record_id = ch
            .record_preset
            .clone()
            .unwrap_or_else(|| encode_id.clone());
        let mut map = self.channels.lock();
        map.entry(ch.id).or_insert_with(|| Chan {
            name: ch.name.clone(),
            encode_preset: encode_id.clone(),
            record_preset: record_id.clone(),
            status: ChannelStatus::Stopped,
            proxy_recording: false,
            hq_recording: false,
            srt: false,
            proxy_recording_path: None,
            hq_recording_path: None,
            srt_url: ch.srt_url.clone(),
            last_error: None,
            udp_egress: ch.udp_egress.clone(),
            started_at: None,
        });
        if let Some(existing) = map.get_mut(&ch.id) {
            existing.name = ch.name.clone();
            existing.encode_preset = ch
                .encode_preset
                .clone()
                .unwrap_or_else(|| encode_id);
            existing.record_preset = ch
                .record_preset
                .clone()
                .unwrap_or_else(|| existing.encode_preset.clone());
            let _ = (encode, record);
            existing.udp_egress = ch.udp_egress.clone();
            if existing.srt_url.is_none() {
                existing.srt_url = ch.srt_url.clone();
            }
        }
        Ok(())
    }

    fn apply_encode_preset(
        &self,
        channel_id: u32,
        preset_id: &str,
        preset: &EncodePreset,
    ) -> Result<()> {
        let mut map = self.channels.lock();
        let ch = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        if ch.proxy_recording || ch.hq_recording {
            bail!("stop recording before changing encode preset");
        }
        if roc_config::is_mezz_codec(&preset.video_codec) {
            bail!("mezz codecs belong on the record preset — pick an NVENC proxy for live/SRT");
        }
        ch.encode_preset = preset_id.to_string();
        Ok(())
    }

    fn apply_record_preset(
        &self,
        channel_id: u32,
        preset_id: &str,
        preset: &EncodePreset,
    ) -> Result<()> {
        let mut map = self.channels.lock();
        let ch = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        // Proxy REC follows the encode preset and may keep running; only HQ blocks.
        if ch.hq_recording {
            bail!("stop recording before changing record preset");
        }
        ch.record_preset = preset_id.to_string();
        let _ = preset;
        Ok(())
    }

    fn start_capture(&self, channel_id: u32) -> Result<()> {
        let mut map = self.channels.lock();
        let ch = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        if matches!(ch.status, ChannelStatus::Running | ChannelStatus::Waiting) {
            return Ok(());
        }
        let used = self.nvenc_used.load(Ordering::SeqCst);
        if used >= self.max_nvenc {
            bail!(
                "NVENC session limit reached ({}/{})",
                used,
                self.max_nvenc
            );
        }
        self.nvenc_used.fetch_add(1, Ordering::SeqCst);
        ch.status = ChannelStatus::Waiting;
        ch.started_at = Some(Instant::now());
        ch.last_error = None;
        tracing::info!(channel_id, "mock capture started (waiting→running)");
        Ok(())
    }

    fn stop_capture(&self, channel_id: u32) -> Result<()> {
        let mut map = self.channels.lock();
        let ch = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        if matches!(ch.status, ChannelStatus::Running | ChannelStatus::Waiting) {
            let prev = self.nvenc_used.fetch_sub(1, Ordering::SeqCst);
            if prev == 0 {
                self.nvenc_used.store(0, Ordering::SeqCst);
            }
        }
        ch.status = ChannelStatus::Stopped;
        ch.proxy_recording = false;
        ch.hq_recording = false;
        ch.srt = false;
        ch.proxy_recording_path = None;
        ch.hq_recording_path = None;
        ch.started_at = None;
        Ok(())
    }

    fn start_recording(&self, channel_id: u32, path: &str) -> Result<()> {
        self.start_hq_recording(channel_id, path)
    }

    fn stop_recording(&self, channel_id: u32) -> Result<()> {
        self.stop_hq_recording(channel_id)
    }

    fn start_proxy_recording(&self, channel_id: u32, path: &str) -> Result<()> {
        self.start_role_recording(channel_id, RecordingRole::Proxy, path)
    }

    fn stop_proxy_recording(&self, channel_id: u32) -> Result<()> {
        self.stop_role_recording(channel_id, RecordingRole::Proxy)
    }

    fn start_hq_recording(&self, channel_id: u32, path: &str) -> Result<()> {
        self.start_role_recording(channel_id, RecordingRole::Hq, path)
    }

    fn stop_hq_recording(&self, channel_id: u32) -> Result<()> {
        self.stop_role_recording(channel_id, RecordingRole::Hq)
    }

    fn start_srt(&self, channel_id: u32, url: &str) -> Result<()> {
        let mut map = self.channels.lock();
        let ch = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        Self::bump_running_status(ch);
        if ch.status != ChannelStatus::Running && ch.status != ChannelStatus::Waiting {
            bail!("capture not running on channel {channel_id}");
        }
        ch.srt = true;
        ch.srt_url = Some(url.to_string());
        Ok(())
    }

    fn stop_srt(&self, channel_id: u32) -> Result<()> {
        let mut map = self.channels.lock();
        let ch = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        ch.srt = false;
        Ok(())
    }

    fn start_webrtc_preview(
        &self,
        _channel_id: u32,
        _pair: u8,
        _signal_tx: crate::PreviewSignalTx,
    ) -> Result<String> {
        bail!("WebRTC preview requires the gst backend")
    }

    fn set_webrtc_answer(&self, _channel_id: u32, _sdp: &str) -> Result<()> {
        bail!("WebRTC preview requires the gst backend")
    }

    fn add_webrtc_ice(
        &self,
        _channel_id: u32,
        _sdp_mline_index: u32,
        _candidate: &str,
    ) -> Result<()> {
        bail!("WebRTC preview requires the gst backend")
    }

    fn stop_webrtc_preview(&self, _channel_id: u32) -> Result<()> {
        Ok(())
    }

    fn stop_webrtc_preview_session(&self, _channel_id: u32, _session_id: &str) -> Result<()> {
        Ok(())
    }

    fn channel_snapshot(&self, channel_id: u32) -> Result<ChannelSnapshot> {
        let mut map = self.channels.lock();
        let ch = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        Self::bump_running_status(ch);
        Ok(self.snapshot_of(channel_id, ch))
    }

    fn list_channels(&self) -> Vec<ChannelSnapshot> {
        let mut map = self.channels.lock();
        let mut ids: Vec<u32> = map.keys().copied().collect();
        ids.sort_unstable();
        ids.into_iter()
            .filter_map(|id| {
                if let Some(ch) = map.get_mut(&id) {
                    Self::bump_running_status(ch);
                }
                let ch = map.get(&id)?;
                Some(self.snapshot_of(id, ch))
            })
            .collect()
    }

    fn nvenc_used(&self) -> usize {
        self.nvenc_used.load(Ordering::SeqCst)
    }

    fn nvenc_limit(&self) -> usize {
        self.max_nvenc
    }

    fn start_playout(&self, client: &PlayoutClientConfig, source: &str) -> Result<()> {
        let mut map = self.playout.lock();
        map.insert(
            client.id.clone(),
            Play {
                name: client.name.clone(),
                device: client.device.clone(),
                status: ChannelStatus::Running,
                source: Some(source.to_string()),
                last_error: None,
            },
        );
        Ok(())
    }

    fn stop_playout(&self, client_id: &str) -> Result<()> {
        let mut map = self.playout.lock();
        if let Some(p) = map.get_mut(client_id) {
            p.status = ChannelStatus::Stopped;
            p.source = None;
        }
        Ok(())
    }

    fn list_playout(&self) -> Vec<PlayoutSnapshot> {
        let map = self.playout.lock();
        let mut out: Vec<_> = map
            .iter()
            .map(|(id, p)| PlayoutSnapshot {
                id: id.clone(),
                name: p.name.clone(),
                status: p.status,
                device: p.device.clone(),
                source: p.source.clone(),
                last_error: p.last_error.clone(),
            })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    fn set_workflow(&self, channel_id: u32, kind: WorkflowKind, active: bool) -> Result<()> {
        // Exclusive: deactivate others on same channel when activating.
        let mut wf = self.workflows.lock();
        if active {
            if let Some(ch) = self.channels.lock().get(&channel_id) {
                if matches!(ch.status, ChannelStatus::Running | ChannelStatus::Waiting) {
                    bail!("stop encode before enabling exclusive workflow on channel {channel_id}");
                }
            }
        }
        wf.insert(channel_id, (kind, active));
        Ok(())
    }

    fn list_workflows(&self) -> Vec<WorkflowSnapshot> {
        let wf = self.workflows.lock();
        let mut out: Vec<_> = wf
            .iter()
            .map(|(id, (kind, active))| WorkflowSnapshot {
                channel_id: *id,
                kind: kind.clone(),
                active: *active,
                detail: Some("stub — TC/commentator pipelines land in Fas 4".into()),
            })
            .collect();
        out.sort_by_key(|w| w.channel_id);
        out
    }
}
