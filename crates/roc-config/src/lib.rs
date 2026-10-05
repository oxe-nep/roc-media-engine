//! Configuration for roc-media-engine (channels, encode presets, paths).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default = "default_recordings_dir")]
    pub recordings_dir: PathBuf,
    #[serde(default = "default_preview_dir")]
    pub preview_dir: PathBuf,
    /// Directory served as `/hls/{channel_id}/…` (GST writes preview here).
    #[serde(default = "default_hls_dir")]
    pub hls_dir: PathBuf,
    /// Public host for SRT listener publish URLs shown in the UI.
    #[serde(default = "default_public_host")]
    pub public_host: String,
    /// Max concurrent NVENC encode sessions (resource limit).
    #[serde(default = "default_max_nvenc")]
    pub max_nvenc_sessions: usize,
    #[serde(default = "default_preset")]
    pub default_encode_preset: String,
    #[serde(default)]
    pub encode_presets: std::collections::HashMap<String, EncodePreset>,
    #[serde(default)]
    pub channels: Vec<ChannelConfig>,
    #[serde(default)]
    pub playout: Vec<PlayoutClientConfig>,
}

fn default_bind() -> String {
    "0.0.0.0:8080".into()
}
fn default_recordings_dir() -> PathBuf {
    PathBuf::from("./recordings")
}
fn default_preview_dir() -> PathBuf {
    PathBuf::from("./preview")
}
fn default_hls_dir() -> PathBuf {
    PathBuf::from("/opt/applications/roc-media-engine/hls")
}
fn default_public_host() -> String {
    "127.0.0.1".into()
}
fn default_max_nvenc() -> usize {
    8
}
fn default_preset() -> String {
    "hq".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncodePreset {
    pub label: String,
    #[serde(default = "default_video_codec")]
    pub video_codec: String,
    pub video_bitrate: String,
    #[serde(default)]
    pub video_maxrate: Option<String>,
    #[serde(default)]
    pub video_bufsize: Option<String>,
    #[serde(default = "default_video_preset")]
    pub video_preset: String,
    #[serde(default = "default_gop")]
    pub video_gop: u32,
    #[serde(default = "default_audio_bitrate")]
    pub audio_bitrate: String,
    #[serde(default = "default_audio_channels")]
    pub audio_channels: u32,
}

impl EncodePreset {
    /// Map FFmpeg-oriented codec/preset ids (from Go UI) onto GST nvh264enc/nvh265enc values.
    pub fn normalize_for_gst(&mut self) {
        self.video_codec = map_video_codec(&self.video_codec);
        self.video_preset = map_nvenc_preset(&self.video_preset);
        if self.video_gop == 0 {
            self.video_gop = default_gop();
        }
        if self.audio_channels == 0 {
            self.audio_channels = default_audio_channels();
        }
        if self.audio_bitrate.trim().is_empty() {
            self.audio_bitrate = default_audio_bitrate();
        }
    }
}

/// FFmpeg / UI codec ids → GST encoder ids used in capture + REC.
pub fn map_video_codec(raw: &str) -> String {
    let c = raw.trim().to_ascii_lowercase();
    if c.is_empty() {
        return default_video_codec();
    }
    // Mezz codecs first — "xavc" contains "avc".
    if c.contains("dnx") {
        return "avenc_dnxhd".into();
    }
    if c.contains("xavc") {
        return "xavc_intra".into();
    }
    if c.contains("265") || c.contains("hevc") {
        "nvh265enc".into()
    } else if c == "nvh264enc"
        || c.contains("264")
        || c == "h264_nvenc"
        || (c.contains("avc") && !c.contains("xavc"))
    {
        "nvh264enc".into()
    } else {
        // Unknown (e.g. av1_nvenc): fall back to H.264 NVENC until supported.
        "nvh264enc".into()
    }
}

/// Mezz codecs encode from the raw tee on REC only; live/SRT stays NVENC.
pub fn is_mezz_codec(video_codec: &str) -> bool {
    let c = video_codec.to_ascii_lowercase();
    c.contains("dnx") || c.contains("xavc")
}

/// File extension for recordings produced with this codec.
pub fn recording_extension(video_codec: &str) -> &'static str {
    let c = video_codec.to_ascii_lowercase();
    if c.contains("dnx") || c.contains("xavc") {
        "mxf"
    } else {
        "mp4"
    }
}

/// FFmpeg NVENC `p1`–`p7` / `llhq` → GST `nvh264enc` preset enum names.
pub fn map_nvenc_preset(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" => default_video_preset(),
        "p1" | "p2" | "hp" => "hp".into(),
        "p3" | "ll" | "llhq" | "low-latency-hq" => "low-latency-hq".into(),
        "llhp" | "low-latency-hp" => "low-latency-hp".into(),
        "low-latency" => "low-latency".into(),
        "p4" | "p5" | "p6" | "p7" | "hq" | "default" => "hq".into(),
        "lossless" => "lossless".into(),
        "lossless-hp" => "lossless-hp".into(),
        other => other.to_string(),
    }
}

fn default_video_codec() -> String {
    "nvh264enc".into()
}
fn default_video_preset() -> String {
    "hq".into()
}
fn default_gop() -> u32 {
    50
}
fn default_audio_bitrate() -> String {
    "192k".into()
}
fn default_audio_channels() -> u32 {
    2
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub encode_preset: Option<String>,
    /// DeckLink device name, e.g. "DeckLink IP 100G (1)"
    pub device: String,
    #[serde(default)]
    pub connection: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    /// Optional UDP MPEG-TS egress for legacy remux clients.
    #[serde(default)]
    pub udp_egress: Option<String>,
    /// Default SRT URL when STREAM is enabled (listener or caller).
    #[serde(default)]
    pub srt_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayoutClientConfig {
    pub id: String,
    pub name: String,
    /// DeckLink sink device name.
    pub device: String,
    #[serde(default)]
    pub format_code: Option<String>,
    #[serde(default)]
    pub srt_url: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("encode preset `{0}` not found")]
    MissingPreset(String),
    #[error("channel `{0}` not found")]
    MissingChannel(u32),
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = fs::read_to_string(path)
            .with_context(|| format!("read config {}", path.display()))?;
        let mut cfg: Config = serde_yaml::from_str(&raw)
            .with_context(|| format!("parse config {}", path.display()))?;
        if cfg.encode_presets.is_empty() {
            cfg.encode_presets = default_presets();
        } else {
            // Merge built-in mezz/NVENC presets without clobbering custom ids.
            for (id, preset) in default_presets() {
                cfg.encode_presets.entry(id).or_insert(preset);
            }
        }
        for p in cfg.encode_presets.values_mut() {
            p.normalize_for_gst();
        }
        Ok(cfg)
    }

    pub fn example() -> Self {
        Self {
            bind: default_bind(),
            recordings_dir: default_recordings_dir(),
            preview_dir: default_preview_dir(),
            hls_dir: default_hls_dir(),
            public_host: default_public_host(),
            max_nvenc_sessions: default_max_nvenc(),
            default_encode_preset: default_preset(),
            encode_presets: default_presets(),
            channels: (1..=8)
                .map(|id| ChannelConfig {
                    id,
                    name: format!("Channel {id}"),
                    encode_preset: Some("hq".into()),
                    device: format!("DeckLink IP 100G ({id})"),
                    connection: None,
                    mode: Some("auto".into()),
                    udp_egress: Some(format!(
                        "udp://239.255.28.{id}:{}?pkt_size=1316&reuse=1&ttl=1",
                        21000 + id
                    )),
                    srt_url: Some(format!("srt://0.0.0.0:{}?mode=listener", 9100 + id)),
                })
                .collect(),
            playout: vec![PlayoutClientConfig {
                id: "decode-1".into(),
                name: "Decode 1".into(),
                device: "DeckLink IP 100G (1)".into(),
                format_code: Some("Hp50".into()),
                srt_url: None,
            }],
        }
    }

    pub fn preset_for_channel(&self, ch: &ChannelConfig) -> Result<&EncodePreset, ConfigError> {
        let id = ch
            .encode_preset
            .as_deref()
            .unwrap_or(&self.default_encode_preset);
        self.encode_presets
            .get(id)
            .ok_or_else(|| ConfigError::MissingPreset(id.to_string()))
    }

    pub fn channel(&self, id: u32) -> Result<&ChannelConfig, ConfigError> {
        self.channels
            .iter()
            .find(|c| c.id == id)
            .ok_or(ConfigError::MissingChannel(id))
    }
}

fn default_presets() -> std::collections::HashMap<String, EncodePreset> {
    let mut m = std::collections::HashMap::new();
    m.insert(
        "proxy".into(),
        EncodePreset {
            label: "Proxy 4 Mbit".into(),
            video_codec: "nvh264enc".into(),
            video_bitrate: "4M".into(),
            video_maxrate: Some("5M".into()),
            video_bufsize: Some("8M".into()),
            video_preset: "low-latency-hq".into(),
            video_gop: 50,
            audio_bitrate: "128k".into(),
            audio_channels: 2,
        },
    );
    m.insert(
        "hq".into(),
        EncodePreset {
            label: "HQ 12 Mbit".into(),
            video_codec: "nvh264enc".into(),
            video_bitrate: "12M".into(),
            video_maxrate: Some("14M".into()),
            video_bufsize: Some("20M".into()),
            video_preset: "low-latency-hq".into(),
            video_gop: 50,
            audio_bitrate: "192k".into(),
            // Stereo default; 8ch (4×AAC) deferred until REC/SRT path is stable.
            audio_channels: 2,
        },
    );
    m.insert(
        "mezz".into(),
        EncodePreset {
            label: "Mezz 20 Mbit".into(),
            video_codec: "nvh264enc".into(),
            video_bitrate: "20M".into(),
            video_maxrate: Some("24M".into()),
            video_bufsize: Some("40M".into()),
            video_preset: "hq".into(),
            video_gop: 50,
            audio_bitrate: "256k".into(),
            audio_channels: 2,
        },
    );
    m.insert(
        "hq_hevc".into(),
        EncodePreset {
            label: "HQ HEVC 8 Mbit".into(),
            video_codec: "nvh265enc".into(),
            video_bitrate: "8M".into(),
            video_maxrate: Some("10M".into()),
            video_bufsize: Some("16M".into()),
            video_preset: "low-latency-hq".into(),
            video_gop: 50,
            audio_bitrate: "192k".into(),
            audio_channels: 2,
        },
    );
    m.insert(
        "mezz_hevc".into(),
        EncodePreset {
            label: "Mezz HEVC 14 Mbit".into(),
            video_codec: "nvh265enc".into(),
            video_bitrate: "14M".into(),
            video_maxrate: Some("16M".into()),
            video_bufsize: Some("28M".into()),
            video_preset: "hq".into(),
            video_gop: 50,
            audio_bitrate: "256k".into(),
            audio_channels: 2,
        },
    );
    // Mezz REC (raw tee + NTP/RTC timecode). Live/SRT still uses NVENC proxy.
    m.insert(
        "dnxhd_145".into(),
        EncodePreset {
            label: "DNxHD 145".into(),
            video_codec: "avenc_dnxhd".into(),
            video_bitrate: "145M".into(),
            video_maxrate: None,
            video_bufsize: None,
            video_preset: "dnxhd".into(),
            video_gop: 1,
            audio_bitrate: "384k".into(),
            audio_channels: 2,
        },
    );
    m.insert(
        "dnxhd_185".into(),
        EncodePreset {
            label: "DNxHD 185".into(),
            video_codec: "avenc_dnxhd".into(),
            video_bitrate: "185M".into(),
            video_maxrate: None,
            video_bufsize: None,
            video_preset: "dnxhd".into(),
            video_gop: 1,
            audio_bitrate: "384k".into(),
            audio_channels: 2,
        },
    );
    m.insert(
        "xavc_intra_hd".into(),
        EncodePreset {
            label: "XAVC Intra HD".into(),
            video_codec: "xavc_intra".into(),
            // ~Class 100 HD target; x264 high-4:2:2-intra approximation.
            video_bitrate: "111M".into(),
            video_maxrate: None,
            video_bufsize: None,
            video_preset: "intra".into(),
            video_gop: 1,
            audio_bitrate: "384k".into(),
            audio_channels: 2,
        },
    );
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_has_eight_channels() {
        let cfg = Config::example();
        assert_eq!(cfg.channels.len(), 8);
        assert!(cfg.encode_presets.contains_key("hq"));
    }

    #[test]
    fn maps_ffmpeg_codec_and_preset() {
        assert_eq!(map_video_codec("h264_nvenc"), "nvh264enc");
        assert_eq!(map_video_codec("hevc_nvenc"), "nvh265enc");
        assert_eq!(map_video_codec("dnxhd"), "avenc_dnxhd");
        assert_eq!(map_video_codec("avenc_dnxhd"), "avenc_dnxhd");
        assert_eq!(map_video_codec("xavc_intra"), "xavc_intra");
        assert!(is_mezz_codec("avenc_dnxhd"));
        assert!(is_mezz_codec("xavc_intra"));
        assert!(!is_mezz_codec("nvh264enc"));
        assert_eq!(recording_extension("avenc_dnxhd"), "mxf");
        assert_eq!(recording_extension("xavc_intra"), "mxf");
        assert_eq!(recording_extension("nvh264enc"), "mp4");
        assert_eq!(map_nvenc_preset("p4"), "hq");
        assert_eq!(map_nvenc_preset("p1"), "hp");
        assert_eq!(map_nvenc_preset("llhq"), "low-latency-hq");
        assert_eq!(map_nvenc_preset("low-latency-hq"), "low-latency-hq");
        let mut p = EncodePreset {
            label: "t".into(),
            video_codec: "h264_nvenc".into(),
            video_bitrate: "12M".into(),
            video_maxrate: None,
            video_bufsize: None,
            video_preset: "p4".into(),
            video_gop: 50,
            audio_bitrate: "192k".into(),
            audio_channels: 2,
        };
        p.normalize_for_gst();
        assert_eq!(p.video_codec, "nvh264enc");
        assert_eq!(p.video_preset, "hq");
    }
}
