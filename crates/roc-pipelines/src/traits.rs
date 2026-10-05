use anyhow::Result;
use roc_config::{ChannelConfig, EncodePreset, PlayoutClientConfig};
use roc_devices::DeviceProbeReport;

use crate::{ChannelSnapshot, ChannelStatus, PlayoutSnapshot, WorkflowKind, WorkflowSnapshot};

/// Backend that owns media graphs for all channels.
pub trait PipelineBackend: Send + Sync {
    fn probe_devices(&self) -> Result<DeviceProbeReport>;

    fn ensure_channel(
        &self,
        ch: &ChannelConfig,
        encode: &EncodePreset,
        record: &EncodePreset,
    ) -> Result<()>;

    /// Hot-swap encode preset (relaunch graph if capture is live). `preset_id` is the config key.
    fn apply_encode_preset(
        &self,
        channel_id: u32,
        preset_id: &str,
        preset: &EncodePreset,
    ) -> Result<()>;

    /// Hot-swap recording preset (no relaunch; applies on next REC start).
    fn apply_record_preset(
        &self,
        channel_id: u32,
        preset_id: &str,
        preset: &EncodePreset,
    ) -> Result<()>;

    fn start_capture(&self, channel_id: u32) -> Result<()>;
    fn stop_capture(&self, channel_id: u32) -> Result<()>;

    fn start_recording(&self, channel_id: u32, path: &str) -> Result<()>;
    fn stop_recording(&self, channel_id: u32) -> Result<()>;

    fn start_srt(&self, channel_id: u32, url: &str) -> Result<()>;
    fn stop_srt(&self, channel_id: u32) -> Result<()>;

    fn channel_snapshot(&self, channel_id: u32) -> Result<ChannelSnapshot>;
    fn list_channels(&self) -> Vec<ChannelSnapshot>;

    fn nvenc_used(&self) -> usize;
    fn nvenc_limit(&self) -> usize;

    fn start_playout(&self, client: &PlayoutClientConfig, source: &str) -> Result<()>;
    fn stop_playout(&self, client_id: &str) -> Result<()>;
    fn list_playout(&self) -> Vec<PlayoutSnapshot>;

    /// Fas 4 stub: apply exclusive workflow (TC / commentator).
    fn set_workflow(&self, channel_id: u32, kind: WorkflowKind, active: bool) -> Result<()>;
    fn list_workflows(&self) -> Vec<WorkflowSnapshot>;
}

/// Shared helpers for bitrate parsing (e.g. "12M" → bits/sec).
pub fn parse_bitrate(s: &str) -> Option<u64> {
    let s = s.trim().to_uppercase();
    if let Some(n) = s.strip_suffix('M') {
        return n.parse::<f64>().ok().map(|v| (v * 1_000_000.0) as u64);
    }
    if let Some(n) = s.strip_suffix('K') {
        return n.parse::<f64>().ok().map(|v| (v * 1_000.0) as u64);
    }
    s.parse().ok()
}

pub fn status_label(s: ChannelStatus) -> &'static str {
    match s {
        ChannelStatus::Stopped => "stopped",
        ChannelStatus::Waiting => "waiting",
        ChannelStatus::Running => "running",
        ChannelStatus::Error => "error",
        ChannelStatus::Restarting => "restarting",
    }
}
