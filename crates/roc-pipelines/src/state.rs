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
    /// Peak levels in dBFS for playout stereo (−90 = silence).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_peaks: Option<Vec<f64>>,
    /// File playhead position in seconds (when known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_sec: Option<f64>,
    /// File duration in seconds (when known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_sec: Option<f64>,
}

/// File transport controls applied to a running playout client.
#[derive(Debug, Clone, Default)]
pub struct PlayoutFileControl {
    pub loop_file: bool,
    pub mark_in_sec: f64,
    /// `None` = play to natural EOF.
    pub mark_out_sec: Option<f64>,
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

/// TC burn-in loop status (DeckLink IN → overlay → DeckLink OUT).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TcLoopStatus {
    Off,
    Running,
    Restarting,
    Error,
}

impl Default for TcLoopStatus {
    fn default() -> Self {
        Self::Off
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TcLoopSource {
    Tod,
    External,
}

impl Default for TcLoopSource {
    fn default() -> Self {
        Self::Tod
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TcLoopPosition {
    BottomRight,
    BottomLeft,
    TopRight,
    TopLeft,
    Center,
}

impl Default for TcLoopPosition {
    fn default() -> Self {
        Self::TopLeft
    }
}

impl TcLoopPosition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BottomRight => "bottom_right",
            Self::BottomLeft => "bottom_left",
            Self::TopRight => "top_right",
            Self::TopLeft => "top_left",
            Self::Center => "center",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "bottom_right" => Self::BottomRight,
            "bottom_left" => Self::BottomLeft,
            "top_right" => Self::TopRight,
            "top_left" => Self::TopLeft,
            "center" => Self::Center,
            _ => Self::TopLeft,
        }
    }

    /// Default normalized top-left anchor for a corner/center preset.
    pub fn default_xy(self) -> (f64, f64) {
        match self {
            Self::TopLeft => (0.04, 0.04),
            Self::TopRight => (0.72, 0.04),
            Self::Center => (0.38, 0.44),
            Self::BottomLeft => (0.04, 0.86),
            Self::BottomRight => (0.72, 0.86),
        }
    }

    /// Nearest preset for a freeform (x, y) — used for legacy `position` field.
    pub fn nearest(x: f64, y: f64) -> Self {
        let pts = [
            Self::TopLeft,
            Self::TopRight,
            Self::Center,
            Self::BottomLeft,
            Self::BottomRight,
        ];
        pts.into_iter()
            .min_by(|a, b| {
                let (ax, ay) = a.default_xy();
                let (bx, by) = b.default_xy();
                let da = (ax - x).hypot(ay - y);
                let db = (bx - x).hypot(by - y);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(Self::TopLeft)
    }
}

impl TcLoopSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tod => "tod",
            Self::External => "external",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "external" => Self::External,
            _ => Self::Tod,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TcLoopSnapshot {
    pub id: u32,
    pub enabled: bool,
    pub status: TcLoopStatus,
    pub source: TcLoopSource,
    pub udp_port: u16,
    pub fontsize: u32,
    pub opacity: f64,
    pub position: TcLoopPosition,
    /// Normalized overlay anchor (0 = left/top, 1 = right/bottom).
    #[serde(default = "default_tc_x")]
    pub x: f64,
    #[serde(default = "default_tc_y")]
    pub y: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timecode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_peaks: Option<Vec<f64>>,
    /// SRT publish armed on the TC proxy encode path.
    #[serde(default)]
    pub srt: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub srt_bitrate_kbps: Option<f64>,
}

fn default_tc_x() -> f64 {
    0.04
}
fn default_tc_y() -> f64 {
    0.04
}
