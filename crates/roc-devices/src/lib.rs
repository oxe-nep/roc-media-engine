//! DeckLink device descriptors and common BMD format codes.

use serde::{Deserialize, Serialize};

/// Known DeckLink format codes used for playout (subset of BMD).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormatCode {
    /// 1080p50
    Hp50,
    /// 1080i50
    Hi50,
    /// 1080p25
    Hp25,
    /// 1080p59.94
    Hp5994,
    /// 1080i59.94
    Hi5994,
}

impl FormatCode {
    pub fn as_bmd_str(self) -> &'static str {
        match self {
            Self::Hp50 => "Hp50",
            Self::Hi50 => "Hi50",
            Self::Hp25 => "Hp25",
            Self::Hp5994 => "Hp59.94",
            Self::Hi5994 => "Hi59.94",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "Hp50" | "hp50" => Some(Self::Hp50),
            "Hi50" | "hi50" => Some(Self::Hi50),
            "Hp25" | "hp25" => Some(Self::Hp25),
            "Hp59.94" | "Hp5994" | "hp5994" => Some(Self::Hp5994),
            "Hi59.94" | "Hi5994" | "hi5994" => Some(Self::Hi5994),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub name: String,
    pub direction: DeviceDirection,
    #[serde(default)]
    pub unique_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceDirection {
    Input,
    Output,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceProbeReport {
    pub backend: String,
    pub devices: Vec<DeviceInfo>,
    pub encoders: Vec<String>,
    pub notes: Vec<String>,
}

impl DeviceProbeReport {
    pub fn empty_mock() -> Self {
        Self {
            backend: "mock".into(),
            devices: (1..=8)
                .flat_map(|id| {
                    [
                        DeviceInfo {
                            name: format!("DeckLink IP 100G ({id})"),
                            direction: DeviceDirection::Input,
                            unique_id: Some(format!("mock-in-{id}")),
                        },
                        DeviceInfo {
                            name: format!("DeckLink IP 100G ({id})"),
                            direction: DeviceDirection::Output,
                            unique_id: Some(format!("mock-out-{id}")),
                        },
                    ]
                })
                .collect(),
            encoders: vec![
                "nvh264enc".into(),
                "nvd3d11h264enc".into(),
                "x264enc".into(),
            ],
            notes: vec![
                "Mock probe — run on capture host with GStreamer + DeckLink plugins for real data."
                    .into(),
            ],
        }
    }
}
