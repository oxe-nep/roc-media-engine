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
    pub recording: bool,
    pub srt: bool,
    pub recording_path: Option<String>,
    pub srt_url: Option<String>,
    pub last_error: Option<String>,
    pub nvenc_slots_used: usize,
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
