//! Map DeckLink/GST caps ↔ mode enum names, and probe live input format.

use serde::{Deserialize, Serialize};

/// Snapshot of what we believe the input is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputFormat {
    pub mode: String,
    pub width: i32,
    pub height: i32,
    pub fps_num: i32,
    pub fps_den: i32,
    pub interlaced: bool,
}

impl InputFormat {
    /// Short UI label, e.g. `1080p50`.
    pub fn summary(&self) -> String {
        if !self.mode.is_empty() {
            return self.mode.clone();
        }
        let scan = if self.interlaced { "i" } else { "p" };
        let fps = if self.fps_den == 1 {
            format!("{}", self.fps_num)
        } else {
            format!("{}/{}", self.fps_num, self.fps_den)
        };
        format!("{}x{}{}{}", self.width, self.height, scan, fps)
    }

    /// Verbose label for logs/tooltips.
    pub fn detail(&self) -> String {
        let scan = if self.interlaced { "i" } else { "p" };
        format!(
            "{}x{}{}{}/{} ({})",
            self.width, self.height, scan, self.fps_num, self.fps_den, self.mode
        )
    }
}

/// Map raw video geometry to a GStreamer `decklinkvideosrc` mode name.
pub fn mode_from_geometry(
    width: i32,
    height: i32,
    fps_num: i32,
    fps_den: i32,
    interlaced: bool,
) -> Option<&'static str> {
    if fps_den <= 0 || fps_num <= 0 {
        return None;
    }
    let fps = fps_num as f64 / fps_den as f64;

    match (width, height, interlaced) {
        (1920, 1080, false) => {
            if approx(fps, 50.0) {
                Some("1080p50")
            } else if approx(fps, 25.0) {
                Some("1080p25")
            } else if approx(fps, 30.0) {
                Some("1080p30")
            } else if approx(fps, 29.97) {
                Some("1080p2997")
            } else if approx(fps, 24.0) {
                Some("1080p24")
            } else if approx(fps, 23.976) {
                Some("1080p2398")
            } else if approx(fps, 60.0) {
                Some("1080p60")
            } else if approx(fps, 59.94) {
                Some("1080p5994")
            } else {
                None
            }
        }
        (1920, 1080, true) => {
            if approx(fps, 25.0) || approx(fps, 50.0) {
                Some("1080i50")
            } else if approx(fps, 29.97) || approx(fps, 59.94) {
                Some("1080i5994")
            } else if approx(fps, 30.0) || approx(fps, 60.0) {
                Some("1080i60")
            } else {
                None
            }
        }
        (1280, 720, false) => {
            if approx(fps, 50.0) {
                Some("720p50")
            } else if approx(fps, 60.0) {
                Some("720p60")
            } else if approx(fps, 59.94) {
                Some("720p5994")
            } else {
                None
            }
        }
        (3840, 2160, false) => {
            if approx(fps, 50.0) {
                Some("2160p50")
            } else if approx(fps, 25.0) {
                Some("2160p25")
            } else if approx(fps, 60.0) {
                Some("2160p60")
            } else {
                None
            }
        }
        _ => None,
    }
}

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.08
}

pub fn is_auto_mode(mode: &str) -> bool {
    let m = mode.trim().to_ascii_lowercase();
    m.is_empty() || m == "auto"
}

/// Map GST DeckLink mode name → BMD playout code used by the UI / `format_code`.
pub fn bmd_code_from_gst_mode(mode: &str) -> &'static str {
    match mode.trim() {
        "1080p50" | "1080i50" => "Hp50",
        "1080p25" => "Hp25",
        "1080p5994" | "1080i5994" => "Hp59.94",
        "1080p60" | "1080i60" => "Hp59.94",
        "720p50" => "Hp50",
        other if other.eq_ignore_ascii_case("Hp50") => "Hp50",
        other if other.eq_ignore_ascii_case("Hi50") => "Hp50",
        other if other.eq_ignore_ascii_case("Hp25") => "Hp25",
        other if other.eq_ignore_ascii_case("Hp59.94") || other.eq_ignore_ascii_case("Hp5994") => {
            "Hp59.94"
        }
        _ => "Hp50",
    }
}

/// Progressive sink mode for playout (interlaced sources → progressive OUT).
pub fn playout_sink_mode_from_input(fmt: &InputFormat) -> String {
    let mode = if fmt.mode.is_empty() {
        mode_from_geometry(fmt.width, fmt.height, fmt.fps_num, fmt.fps_den, fmt.interlaced)
            .unwrap_or("1080p50")
            .to_string()
    } else {
        fmt.mode.clone()
    };
    match mode.as_str() {
        "1080i50" => "1080p50".into(),
        "1080i5994" => "1080p5994".into(),
        "1080i60" => "1080p60".into(),
        other => other.to_string(),
    }
}

/// Resolve format_code for playout: explicit override, else probe, else Hp50.
pub fn resolve_playout_format_code(configured: Option<&str>, source: &str) -> String {
    let cfg = configured.unwrap_or("auto").trim();
    if !is_auto_mode(cfg) {
        return if let Some(fc) = roc_devices::FormatCode::parse(cfg) {
            fc.as_bmd_str().to_string()
        } else {
            cfg.to_string()
        };
    }
    #[cfg(feature = "gst")]
    {
        match probe_playout_source(source, 4000) {
            Ok(fmt) => {
                let sink = playout_sink_mode_from_input(&fmt);
                bmd_code_from_gst_mode(&sink).to_string()
            }
            Err(err) => {
                tracing::warn!(error = %err, %source, "playout format probe failed — falling back to Hp50");
                "Hp50".into()
            }
        }
    }
    #[cfg(not(feature = "gst"))]
    {
        let _ = source;
        "Hp50".into()
    }
}

#[cfg(feature = "gst")]
mod gst_probe {
    use super::*;
    use anyhow::{anyhow, Context, Result};
    use gstreamer::prelude::*;

    use crate::describe::decklink_device_number;

    pub fn format_from_caps(caps: &gstreamer::Caps) -> Option<InputFormat> {
        let s = caps.structure(0)?;
        let width = s.get::<i32>("width").ok()?;
        let height = s.get::<i32>("height").ok()?;
        let fr = s.get::<gstreamer::Fraction>("framerate").ok()?;
        let fps_num = fr.numer();
        let fps_den = fr.denom();
        let fps = fps_num as f64 / fps_den as f64;
        let interlaced = match s.get::<&str>("interlace-mode") {
            Ok("progressive") | Ok("progressive-frame") => false,
            Ok(_) => true,
            Err(_) => {
                // DeckLink sometimes omits interlace-mode. Broadcast 1080@25 is almost always i50.
                width == 1920 && height == 1080 && (fps - 25.0).abs() < 0.5
            }
        };
        let mode = mode_from_geometry(width, height, fps_num, fps_den, interlaced)?.to_string();
        Some(InputFormat {
            mode,
            width,
            height,
            fps_num,
            fps_den,
            interlaced,
        })
    }

    /// Brief probe: open DeckLink in `mode=auto`, wait for stable caps, map to mode.
    pub fn probe_input_format(device: &str, timeout_ms: u64) -> Result<InputFormat> {
        let dev = decklink_device_number(device);
        let launch = format!(
            "decklinkvideosrc name=dlsrc device-number={dev} mode=auto drop-no-signal-frames=false ! fakesink sync=false"
        );
        let pipeline = gstreamer::parse::launch(&launch)
            .context("parse probe pipeline")?
            .downcast::<gstreamer::Pipeline>()
            .map_err(|_| anyhow!("probe launch not a Pipeline"))?;

        let dl = pipeline
            .by_name("dlsrc")
            .ok_or_else(|| anyhow!("dlsrc missing in probe"))?;
        let pad = dl.static_pad("src").ok_or_else(|| anyhow!("no src pad"))?;

        pipeline
            .set_state(gstreamer::State::Playing)
            .context("probe PLAYING")?;

        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        let mut best: Option<InputFormat> = None;
        while std::time::Instant::now() < deadline {
            if let Some(caps) = pad.current_caps() {
                if let Some(fmt) = format_from_caps(&caps) {
                    // Prefer HD; if we ever see interlaced for the same geometry, keep that
                    // (DeckLink often flickers progressive first on i50).
                    let take = match &best {
                        None => true,
                        Some(prev) if fmt.width >= 1280 && prev.width < 1280 => true,
                        Some(prev)
                            if fmt.width == prev.width
                                && fmt.height == prev.height
                                && fmt.interlaced
                                && !prev.interlaced =>
                        {
                            true
                        }
                        Some(prev) if fmt.width >= 1280 && prev.width >= 1280 => false,
                        Some(_) => fmt.width >= 1280,
                    };
                    if take {
                        best = Some(fmt.clone());
                    }
                    // Only stop early once we have a confident interlaced HD lock.
                    if fmt.width >= 1280 && fmt.interlaced {
                        break;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        let _ = pipeline.set_state(gstreamer::State::Null);
        best.ok_or_else(|| anyhow!("no input caps on {device} within {timeout_ms}ms"))
    }

    /// Probe file or SRT source video geometry (decode to raw caps, no DeckLink).
    pub fn probe_playout_source(source: &str, timeout_ms: u64) -> Result<InputFormat> {
        let src = if source.starts_with("srt://") {
            format!("srtsrc uri=\"{source}\" ! tsdemux name=d")
        } else if source.ends_with(".ts") {
            format!("filesrc location=\"{source}\" ! tsdemux name=d")
        } else {
            format!("filesrc location=\"{source}\" ! qtdemux name=d")
        };
        let launch = format!(
            "{src} \
             d. ! queue ! video/x-h264 ! h264parse ! avdec_h264 ! videoconvert ! \
             video/x-raw ! fakesink name=vsink sync=false"
        );
        let pipeline = gstreamer::parse::launch(&launch)
            .context("parse playout probe pipeline")?
            .downcast::<gstreamer::Pipeline>()
            .map_err(|_| anyhow!("playout probe launch not a Pipeline"))?;

        let sink = pipeline
            .by_name("vsink")
            .ok_or_else(|| anyhow!("vsink missing in playout probe"))?;
        let pad = sink
            .static_pad("sink")
            .ok_or_else(|| anyhow!("no sink pad on vsink"))?;

        pipeline
            .set_state(gstreamer::State::Playing)
            .context("playout probe PLAYING")?;

        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        let mut best: Option<InputFormat> = None;
        while std::time::Instant::now() < deadline {
            if let Some(caps) = pad.current_caps() {
                if let Some(fmt) = format_from_caps(&caps) {
                    best = Some(fmt);
                    break;
                }
            }
            // Also try peer caps if sink has not negotiated yet.
            if let Some(peer) = pad.peer() {
                if let Some(caps) = peer.current_caps() {
                    if let Some(fmt) = format_from_caps(&caps) {
                        best = Some(fmt);
                        break;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        let _ = pipeline.set_state(gstreamer::State::Null);
        best.ok_or_else(|| anyhow!("no video caps from playout source within {timeout_ms}ms"))
    }
}

#[cfg(feature = "gst")]
pub use gst_probe::{format_from_caps, probe_input_format, probe_playout_source};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_1080p50() {
        assert_eq!(
            mode_from_geometry(1920, 1080, 50, 1, false),
            Some("1080p50")
        );
    }

    #[test]
    fn maps_1080i50() {
        assert_eq!(
            mode_from_geometry(1920, 1080, 25, 1, true),
            Some("1080i50")
        );
    }

    #[test]
    fn auto_mode() {
        assert!(is_auto_mode("auto"));
        assert!(is_auto_mode(""));
        assert!(!is_auto_mode("1080p50"));
    }

    #[test]
    fn bmd_from_gst_maps_interlace_to_hp50() {
        assert_eq!(bmd_code_from_gst_mode("1080i50"), "Hp50");
        assert_eq!(bmd_code_from_gst_mode("1080p50"), "Hp50");
    }

    #[test]
    fn playout_sink_forces_progressive() {
        let fmt = InputFormat {
            mode: "1080i50".into(),
            width: 1920,
            height: 1080,
            fps_num: 25,
            fps_den: 1,
            interlaced: true,
        };
        assert_eq!(playout_sink_mode_from_input(&fmt), "1080p50");
    }
}
