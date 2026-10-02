use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelStatus {
    Stopped,
    Waiting,
    Running,
    Error,
    Restarting,
}

impl Default for ChannelStatus {
    fn default() -> Self {
        Self::Stopped
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelSnapshot {
    pub id: u32,
    pub name: String,
    pub status: ChannelStatus,
    pub encode_preset: String,
    /// Live encoded video bitrate (kbps) from pad probe — omit until first sample.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_bitrate_kbps: Option<f64>,
    /// Live SRT MPEG-TS bitrate (kbps) while SRT branch is attached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub srt_bitrate_kbps: Option<f64>,
    pub recording: bool,
    pub srt: bool,
    pub recording_path: Option<String>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayoutSnapshot {
    pub id: String,
    pub name: String,
    pub status: ChannelStatus,
    pub device: String,
    pub source: Option<String>,
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
