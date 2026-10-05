use anyhow::Result;
use roc_config::{ChannelConfig, EncodePreset, PlayoutClientConfig};
use roc_devices::DeviceProbeReport;

use crate::{
    ChannelSnapshot, ChannelStatus, PlayoutFileControl, PlayoutSnapshot, WorkflowKind,
    WorkflowSnapshot,
};

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

    /// Backward-compatible alias for [`Self::start_hq_recording`].
    fn start_recording(&self, channel_id: u32, path: &str) -> Result<()>;
    /// Backward-compatible alias for [`Self::stop_hq_recording`].
    fn stop_recording(&self, channel_id: u32) -> Result<()>;

    /// Proxy REC: encoded bitstream from tee `e` (encode preset codec) → .mp4.
    fn start_proxy_recording(&self, channel_id: u32, path: &str) -> Result<()>;
    fn stop_proxy_recording(&self, channel_id: u32) -> Result<()>;

    /// HQ REC: record preset (mezz from raw tee `t`, else encoded from tee `e`).
    fn start_hq_recording(&self, channel_id: u32, path: &str) -> Result<()>;
    fn stop_hq_recording(&self, channel_id: u32) -> Result<()>;

    fn start_srt(&self, channel_id: u32, url: &str) -> Result<()>;
    fn stop_srt(&self, channel_id: u32) -> Result<()>;

    /// On-demand WebRTC encode preview (sendonly). `pair` is stereo pair 0..=3.
    fn start_webrtc_preview(
        &self,
        channel_id: u32,
        pair: u8,
        signal_tx: crate::PreviewSignalTx,
    ) -> Result<String>;
    fn set_webrtc_answer(&self, channel_id: u32, sdp: &str) -> Result<()>;
    fn add_webrtc_ice(&self, channel_id: u32, sdp_mline_index: u32, candidate: &str) -> Result<()>;
    fn stop_webrtc_preview(&self, channel_id: u32) -> Result<()>;
    /// Stop only if `session_id` still owns the channel preview (avoids killing a handoff).
    fn stop_webrtc_preview_session(&self, channel_id: u32, session_id: &str) -> Result<()>;

    fn channel_snapshot(&self, channel_id: u32) -> Result<ChannelSnapshot>;
    fn list_channels(&self) -> Vec<ChannelSnapshot>;

    fn nvenc_used(&self) -> usize;
    fn nvenc_limit(&self) -> usize;

    fn start_playout(&self, client: &PlayoutClientConfig, source: &str) -> Result<()>;
    fn stop_playout(&self, client_id: &str) -> Result<()>;
    fn pause_playout(&self, client_id: &str) -> Result<()>;
    fn resume_playout(&self, client_id: &str) -> Result<()>;
    /// Seek file playout to `position_sec` (clamped to marks when set).
    fn seek_playout(&self, client_id: &str, position_sec: f64) -> Result<()>;
    /// Update loop / in-out marks on a running file playout (no-op if not running).
    fn set_playout_file_control(&self, client_id: &str, control: &PlayoutFileControl) -> Result<()>;
    fn list_playout(&self) -> Vec<PlayoutSnapshot>;

    /// TC burn-in: DeckLink IN → overlay → DeckLink OUT.
    fn start_tc_loop(&self, channel_id: u32, opts: &crate::TcLoopLaunchOpts) -> Result<()>;
    fn stop_tc_loop(&self, channel_id: u32) -> Result<()>;
    /// Hot-update overlay placement / font without relaunching the pipeline.
    fn update_tc_overlay(
        &self,
        channel_id: u32,
        fontsize: u32,
        opacity: f64,
        x: f64,
        y: f64,
        position: crate::TcLoopPosition,
    ) -> Result<()>;
    fn list_tc_loops(&self) -> Vec<crate::TcLoopSnapshot>;
    fn tc_loop_snapshot(&self, channel_id: u32) -> Option<crate::TcLoopSnapshot>;

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
        ChannelStatus::Paused => "paused",
        ChannelStatus::Error => "error",
        ChannelStatus::Restarting => "restarting",
    }
}
