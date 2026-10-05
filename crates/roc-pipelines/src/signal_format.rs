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
        let interlaced = match s.get::<&str>("interlace-mode") {
            Ok("progressive") | Ok("progressive-frame") => false,
            Ok(_) => true,
            Err(_) => false,
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
                    // Prefer HD over the brief SD lock-on that DeckLink often emits first.
                    if fmt.width >= 1280 {
                        best = Some(fmt);
                        break;
                    }
                    best = Some(fmt);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        let _ = pipeline.set_state(gstreamer::State::Null);
        best.ok_or_else(|| anyhow!("no input caps on {device} within {timeout_ms}ms"))
    }
}

#[cfg(feature = "gst")]
pub use gst_probe::{format_from_caps, probe_input_format};

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
}
