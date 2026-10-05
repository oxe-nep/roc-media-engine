use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelStatus {
    Stopped,
    Waiting,
    Running,
    /// File playout paused (GST PAUSED).
    Paused,
    Error,
    Restarting,
}

impl Default for ChannelStatus {
    fn default() -> Self {
        Self::Stopped
    }
}

/// Which file recording a request targets. A channel can record both at once.
///
/// - `Proxy`: always the encoded bitstream from tee `e` (encode preset codec) → `.mp4`.
/// - `Hq`: the record preset — mezz from raw tee `t`, otherwise encoded from tee `e`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingRole {
    Proxy,
    Hq,
}

impl RecordingRole {
    /// Short tag used in GStreamer element names and logs (`proxy` / `hq`).
    pub fn tag(self) -> &'static str {
        match self {
            Self::Proxy => "proxy",
            Self::Hq => "hq",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelSnapshot {
    pub id: u32,
    pub name: String,
    pub status: ChannelStatus,
    pub encode_preset: String,
    /// Preset used for file REC (may differ from live/proxy encode_preset).
    #[serde(default)]
    pub record_preset: String,
    /// Live encoded video bitrate (kbps) from pad probe — omit until first sample.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_bitrate_kbps: Option<f64>,
    /// Live SRT MPEG-TS bitrate (kbps) while SRT branch is attached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub srt_bitrate_kbps: Option<f64>,
    /// True when either the proxy or the HQ recording is active (backward compat).
    pub recording: bool,
    /// Proxy recording (encoded tee `e` → .mp4) is active.
    #[serde(default)]
    pub proxy_recording: bool,
    /// HQ recording (record preset: mezz from raw tee or encoded) is active.
    #[serde(default)]
    pub hq_recording: bool,
    pub srt: bool,
    /// HQ file path if HQ is recording, otherwise the proxy path (backward compat).
    pub recording_path: Option<String>,
    #[serde(default)]
    pub proxy_recording_path: Option<String>,
    #[serde(default)]
    pub hq_recording_path: Option<String>,
    pub srt_url: Option<String>,
    pub last_error: Option<String>,
    pub nvenc_slots_used: usize,
    /// Configured mode (`auto` or locked enum name).
    #[serde(default)]
    pub configured_mode: String,
    /// Mode currently locked into the capture graph.
    #[serde(default)]
    pub locked_mode: Option<String>,
    /// Last detected input format summary (from live caps).
    #[serde(default)]
    pub input_format: Option<String>,
    /// Peak levels in dBFS for up to 8 discrete channels (−90 = silence).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_peaks: Option<Vec<f64>>,
    /// Bumps on each capture graph launch so the UI remounts HLS after relaunch.
    #[serde(default)]
    pub preview_epoch: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayoutSnapshot {
    pub id: String,
    pub name: String,
    pub status: ChannelStatus,
    pub device: String,
    pub source: Option<String>,
    /// Locked BMD/GST format code after probe (e.g. `Hp50`), when known.
    #[serde(default)]
    pub format_code: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkflowKind {
    #[serde(rename = "pair")]
    Pair,
    #[serde(rename = "tc")]
    Timecode,
    #[serde(rename = "commentator")]
    Commentator,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowSnapshot {
    pub channel_id: u32,
    pub kind: WorkflowKind,
    pub active: bool,
    pub detail: Option<String>,
}
