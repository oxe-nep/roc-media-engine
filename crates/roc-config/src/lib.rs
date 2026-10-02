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
    "0.0.0.0:8090".into()
}
fn default_recordings_dir() -> PathBuf {
    PathBuf::from("./recordings")
}
fn default_preview_dir() -> PathBuf {
    PathBuf::from("./preview")
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
        }
        Ok(cfg)
    }

    pub fn example() -> Self {
        Self {
            bind: default_bind(),
            recordings_dir: default_recordings_dir(),
            preview_dir: default_preview_dir(),
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
                    mode: None,
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
}
