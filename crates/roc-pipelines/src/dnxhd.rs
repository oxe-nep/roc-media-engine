//! Avid DNxHD / VC-3 operating points for mezz REC.
//!
//! Supported rasters for this deployment: **1080i50** and **1080p50** only.
//! Preset selects class (SQ / HQ / HQX); bitrate follows the live signal.

use anyhow::{bail, Result};

use crate::signal_format::InputFormat;

/// DNxHD quality class (maps to Avid SQ / HQ / HQX families).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnxhdClass {
    /// Standard Quality — 8-bit 4:2:2 (120 @ i50 / 240 @ p50).
    Sq,
    /// High Quality — 8-bit 4:2:2 (185 @ i50 / 365 @ p50).
    Hq,
    /// High Quality 10-bit — HQX (185x @ i50 / 365x @ p50).
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
        if p == "sq" || p.contains("_sq") || p.contains("120") || p.contains("240") {
            return Self::Sq;
        }
        if p == "hq" || p.contains("_hq") || p.contains("185") || p.contains("365") {
            return Self::Hq;
        }
        // Legacy ids / labels.
        if p.contains("145") {
            return Self::Sq;
        }
        if p == "dnxhd" || p.is_empty() {
            if let Some(br) = bitrate_hint {
                let mbps = br / 1_000_000;
                if matches!(mbps, 120 | 240 | 145) {
                    return Self::Sq;
                }
                if matches!(mbps, 185 | 365) {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DnxhdRaster {
    /// 1080i50 — frame rate 25/1, interlaced.
    I50,
    /// 1080p50 — frame rate 50/1, progressive.
    P50,
}

fn classify_raster(fmt: &InputFormat) -> Result<DnxhdRaster> {
    if fmt.width < 1920 || fmt.height < 1080 {
        bail!(
            "DNxHD requires 1920x1080 (got {}x{})",
            fmt.width,
            fmt.height
        );
    }
    if fmt.interlaced {
        if approx_fps(fmt.fps_num, fmt.fps_den, 25.0) || approx_fps(fmt.fps_num, fmt.fps_den, 50.0)
        {
            return Ok(DnxhdRaster::I50);
        }
        bail!(
            "DNxHD only supports 1080i50 and 1080p50 (got interlaced {}/{})",
            fmt.fps_num,
            fmt.fps_den
        );
    }
    if approx_fps(fmt.fps_num, fmt.fps_den, 50.0) {
        return Ok(DnxhdRaster::P50);
    }
    bail!(
        "DNxHD only supports 1080i50 and 1080p50 (got progressive {}/{})",
        fmt.fps_num,
        fmt.fps_den
    );
}

fn bitrate_for(class: DnxhdClass, raster: DnxhdRaster) -> (u64, &'static str) {
    match (raster, class) {
        (DnxhdRaster::I50, DnxhdClass::Sq) => (120_000_000, "DNxHD 120"),
        (DnxhdRaster::I50, DnxhdClass::Hq) => (185_000_000, "DNxHD 185"),
        (DnxhdRaster::I50, DnxhdClass::Hqx) => (185_000_000, "DNxHD 185x"),
        (DnxhdRaster::P50, DnxhdClass::Sq) => (240_000_000, "DNxHD 240"),
        (DnxhdRaster::P50, DnxhdClass::Hq) => (365_000_000, "DNxHD 365"),
        (DnxhdRaster::P50, DnxhdClass::Hqx) => (365_000_000, "DNxHD 365x"),
    }
}

/// Resolve DNxHD encode parameters from live format + preset class.
pub fn resolve_dnxhd(fmt: &InputFormat, class: DnxhdClass) -> Result<DnxhdOperatingPoint> {
    let raster = classify_raster(fmt)?;
    let (fps_num, fps_den, interlaced) = match raster {
        DnxhdRaster::I50 => (25, 1, true),
        DnxhdRaster::P50 => (50, 1, false),
    };
    let (bitrate, tag) = bitrate_for(class, raster);
    let raw_format = match class {
        DnxhdClass::Sq | DnxhdClass::Hq => "Y42B",
        DnxhdClass::Hqx => "v210",
    };
    Ok(DnxhdOperatingPoint {
        class,
        bitrate,
        label: tag.into(),
        width: fmt.width.max(1920),
        height: fmt.height.max(1080),
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
            bit_depth: 8,
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
            bit_depth: 8,
        }
    }

    fn fmt_p50() -> InputFormat {
        InputFormat {
            mode: "1080p50".into(),
            width: 1920,
            height: 1080,
            fps_num: 50,
            fps_den: 1,
            interlaced: false,
            bit_depth: 8,
        }
    }

    fn fmt_i5994() -> InputFormat {
        InputFormat {
            mode: "1080i5994".into(),
            width: 1920,
            height: 1080,
            fps_num: 30000,
            fps_den: 1001,
            interlaced: true,
            bit_depth: 8,
        }
    }

    #[test]
    fn i50_hq_is_185_interlaced() {
        let op = resolve_dnxhd(&fmt_i50(), DnxhdClass::Hq).unwrap();
        assert_eq!(op.bitrate, 185_000_000);
        assert_eq!(op.fps_num, 25);
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
    fn p50_hq_is_365_progressive() {
        let op = resolve_dnxhd(&fmt_p50(), DnxhdClass::Hq).unwrap();
        assert_eq!(op.bitrate, 365_000_000);
        assert_eq!(op.fps_num, 50);
        assert!(!op.interlaced);
        assert!(!op.video_caps().contains("interlace-mode"));
    }

    #[test]
    fn i50_hqx_requests_v210() {
        let op = resolve_dnxhd(&fmt_i50(), DnxhdClass::Hqx).unwrap();
        assert_eq!(op.raw_format, "v210");
        assert!(op.label.contains('x'));
    }

    #[test]
    fn i5994_is_rejected() {
        let err = resolve_dnxhd(&fmt_i5994(), DnxhdClass::Hq).unwrap_err();
        assert!(err.to_string().contains("1080i50"));
    }

    #[test]
    fn parse_class_from_legacy_preset() {
        assert_eq!(
            DnxhdClass::parse("dnxhd", Some(120_000_000)),
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
