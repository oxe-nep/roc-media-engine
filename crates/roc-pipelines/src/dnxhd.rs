//! Avid DNxHD / VC-3 operating points for mezz REC.
//!
//! Preset selects **class** (SQ / HQ / HQX). Bitrate and pixel format come from
//! the live signal geometry so 1080i50 stays interlaced at the correct Mbps.

use anyhow::{bail, Result};

use crate::signal_format::InputFormat;

/// DNxHD quality class (maps to Avid SQ / HQ / HQX families).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnxhdClass {
    /// Standard Quality — 8-bit 4:2:2 (e.g. 120 @ 1080i50).
    Sq,
    /// High Quality — 8-bit 4:2:2 (e.g. 185 @ 1080i50).
    Hq,
    /// High Quality 10-bit — HQX (e.g. 185x @ 1080i50).
    Hqx,
}

impl DnxhdClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sq => "sq",
            Self::Hq => "hq",
            Self::Hqx => "hqx",
        }
    }

    pub fn bits(self) -> u8 {
        match self {
            Self::Sq | Self::Hq => 8,
            Self::Hqx => 10,
        }
    }

    /// Parse from preset `video_preset` / id / bitrate hint.
    pub fn parse(preset: &str, bitrate_hint: Option<u64>) -> Self {
        let p = preset.trim().to_ascii_lowercase();
        if p.contains("hqx") || (p.ends_with('x') && p.contains("dnx")) {
            return Self::Hqx;
        }
        if p == "sq" || p.contains("_sq") || p.contains("145") || p.contains("120") {
            return Self::Sq;
        }
        if p == "hq" || p.contains("_hq") || p.contains("185") || p.contains("220") {
            return Self::Hq;
        }
        if p == "dnxhd" || p.is_empty() {
            // Legacy single preset — prefer HQ unless bitrate looks like SQ.
            if let Some(br) = bitrate_hint {
                let mbps = br / 1_000_000;
                if matches!(mbps, 115 | 120 | 145 | 240) {
                    return Self::Sq;
                }
                if matches!(mbps, 175 | 185 | 220 | 365) {
                    return Self::Hq;
                }
            }
            return Self::Hq;
        }
        Self::Hq
    }
}

/// Resolved encode parameters for one DNxHD recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnxhdOperatingPoint {
    pub class: DnxhdClass,
    /// Nominal Avid label bitrate in bit/s (e.g. 185_000_000).
    pub bitrate: u64,
    /// Short label for logs / UI, e.g. `DNxHD 185`.
    pub label: String,
    pub width: i32,
    pub height: i32,
    pub fps_num: i32,
    pub fps_den: i32,
    pub interlaced: bool,
    /// GStreamer raw format into `avenc_dnxhd`: `Y42B` (8-bit) or `v210` (10-bit).
    pub raw_format: &'static str,
}

impl DnxhdOperatingPoint {
    pub fn bits(&self) -> u8 {
        self.class.bits()
    }

    /// Caps string for the mezz capsfilter (no width/height — convert negotiates).
    pub fn video_caps(&self) -> String {
        let mut caps = format!(
            "video/x-raw,format={},framerate={}/{}",
            self.raw_format, self.fps_num, self.fps_den
        );
        if self.interlaced {
            caps.push_str(",interlace-mode=interleaved");
        }
        caps
    }
}

fn approx_fps(num: i32, den: i32, target: f64) -> bool {
    if den <= 0 {
        return false;
    }
    ((num as f64 / den as f64) - target).abs() < 0.08
}

/// Frame rate + scan for DNxHD (field rate collapsed to frame rate for interlaced).
fn dnxhd_timebase(fmt: &InputFormat) -> Result<(i32, i32, bool)> {
    let interlaced = fmt.interlaced;
    if interlaced {
        if approx_fps(fmt.fps_num, fmt.fps_den, 25.0) || approx_fps(fmt.fps_num, fmt.fps_den, 50.0)
        {
            return Ok((25, 1, true));
        }
        if approx_fps(fmt.fps_num, fmt.fps_den, 29.97)
            || approx_fps(fmt.fps_num, fmt.fps_den, 59.94)
        {
            return Ok((30000, 1001, true));
        }
        if approx_fps(fmt.fps_num, fmt.fps_den, 30.0) || approx_fps(fmt.fps_num, fmt.fps_den, 60.0)
        {
            return Ok((30, 1, true));
        }
        bail!(
            "unsupported interlaced rate {}/{} for DNxHD",
            fmt.fps_num,
            fmt.fps_den
        );
    }
    Ok((fmt.fps_num.max(1), fmt.fps_den.max(1), false))
}

/// Mbps for (class, interlaced, fps family) at 1920x1080.
fn bitrate_mbps_1080(
    class: DnxhdClass,
    interlaced: bool,
    fps_num: i32,
    fps_den: i32,
) -> Result<(u64, &'static str)> {
    let fps = fps_num as f64 / fps_den.max(1) as f64;
    let family_25 = approx_fps(fps_num, fps_den, 25.0);
    let family_30 = approx_fps(fps_num, fps_den, 29.97) || approx_fps(fps_num, fps_den, 30.0);
    let family_50p = !interlaced && approx_fps(fps_num, fps_den, 50.0);
    let family_60p =
        !interlaced && (approx_fps(fps_num, fps_den, 59.94) || approx_fps(fps_num, fps_den, 60.0));

    let (mbps, tag) = match class {
        DnxhdClass::Sq => {
            if family_50p {
                (240, "DNxHD 240")
            } else if family_60p {
                (290, "DNxHD 290")
            } else if family_30 {
                (145, "DNxHD 145")
            } else if family_25 || interlaced {
                (120, "DNxHD 120")
            } else {
                bail!("unsupported progressive rate {fps:.3} for DNxHD SQ");
            }
        }
        DnxhdClass::Hq => {
            if family_50p {
                (365, "DNxHD 365")
            } else if family_60p {
                (440, "DNxHD 440")
            } else if family_30 {
                (220, "DNxHD 220")
            } else if family_25 || interlaced {
                (185, "DNxHD 185")
            } else {
                bail!("unsupported progressive rate {fps:.3} for DNxHD HQ");
            }
        }
        DnxhdClass::Hqx => {
            if family_50p {
                (365, "DNxHD 365x")
            } else if family_60p {
                (440, "DNxHD 440x")
            } else if family_30 {
                (220, "DNxHD 220x")
            } else if family_25 || interlaced {
                (185, "DNxHD 185x")
            } else {
                bail!("unsupported progressive rate {fps:.3} for DNxHD HQX");
            }
        }
    };
    Ok((mbps * 1_000_000, tag))
}

/// Resolve DNxHD encode parameters from live format + preset class.
pub fn resolve_dnxhd(fmt: &InputFormat, class: DnxhdClass) -> Result<DnxhdOperatingPoint> {
    if fmt.width < 1280 || fmt.height < 720 {
        bail!(
            "DNxHD requires HD geometry (got {}x{})",
            fmt.width,
            fmt.height
        );
    }
    let (fps_num, fps_den, interlaced) = dnxhd_timebase(fmt)?;
    let (bitrate, tag) = bitrate_mbps_1080(class, interlaced, fps_num, fps_den)?;
    let raw_format = match class {
        DnxhdClass::Sq | DnxhdClass::Hq => "Y42B",
        DnxhdClass::Hqx => "v210",
    };
    Ok(DnxhdOperatingPoint {
        class,
        bitrate,
        label: tag.into(),
        width: fmt.width,
        height: fmt.height,
        fps_num,
        fps_den,
        interlaced,
        raw_format,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt_i50() -> InputFormat {
        InputFormat {
            mode: "1080i50".into(),
            width: 1920,
            height: 1080,
            fps_num: 25,
            fps_den: 1,
            interlaced: true,
        }
    }

    fn fmt_i50_field_rate() -> InputFormat {
        InputFormat {
            mode: "1080i50".into(),
            width: 1920,
            height: 1080,
            fps_num: 50,
            fps_den: 1,
            interlaced: true,
        }
    }

    #[test]
    fn i50_hq_is_185_interlaced_no_fps_doubling() {
        let op = resolve_dnxhd(&fmt_i50(), DnxhdClass::Hq).unwrap();
        assert_eq!(op.bitrate, 185_000_000);
        assert_eq!(op.fps_num, 25);
        assert_eq!(op.fps_den, 1);
        assert!(op.interlaced);
        assert_eq!(op.raw_format, "Y42B");
        assert!(op.video_caps().contains("interlace-mode=interleaved"));
    }

    #[test]
    fn i50_field_rate_still_25_frame() {
        let op = resolve_dnxhd(&fmt_i50_field_rate(), DnxhdClass::Sq).unwrap();
        assert_eq!(op.bitrate, 120_000_000);
        assert_eq!(op.fps_num, 25);
        assert!(op.interlaced);
    }

    #[test]
    fn i50_hqx_requests_v210() {
        let op = resolve_dnxhd(&fmt_i50(), DnxhdClass::Hqx).unwrap();
        assert_eq!(op.raw_format, "v210");
        assert_eq!(op.bitrate, 185_000_000);
        assert!(op.label.contains('x'));
    }

    #[test]
    fn parse_class_from_legacy_preset() {
        assert_eq!(
            DnxhdClass::parse("dnxhd", Some(145_000_000)),
            DnxhdClass::Sq
        );
        assert_eq!(
            DnxhdClass::parse("dnxhd", Some(185_000_000)),
            DnxhdClass::Hq
        );
        assert_eq!(DnxhdClass::parse("hqx", None), DnxhdClass::Hqx);
        assert_eq!(DnxhdClass::parse("sq", None), DnxhdClass::Sq);
    }
}
