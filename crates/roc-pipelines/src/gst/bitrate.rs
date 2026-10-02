//! Sliding-window bitrate from pad probes (bytes → kbps).

use std::sync::Arc;
use std::time::Instant;

use gstreamer::prelude::*;
use parking_lot::Mutex;

struct Inner {
    window_start: Instant,
    bytes: u64,
    last_kbps: f64,
    have_sample: bool,
}

/// Shared meter updated from a GST buffer pad probe.
#[derive(Clone)]
pub struct BitrateMeter {
    inner: Arc<Mutex<Inner>>,
}

impl BitrateMeter {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                window_start: Instant::now(),
                bytes: 0,
                last_kbps: 0.0,
                have_sample: false,
            })),
        }
    }

    pub fn reset(&self) {
        let mut g = self.inner.lock();
        g.window_start = Instant::now();
        g.bytes = 0;
        g.last_kbps = 0.0;
        g.have_sample = false;
    }

    pub fn note_bytes(&self, n: u64) {
        if n == 0 {
            return;
        }
        let mut g = self.inner.lock();
        g.bytes = g.bytes.saturating_add(n);
        let elapsed = g.window_start.elapsed().as_secs_f64();
        // ~1s windows for stable UI (matches FFmpeg progress cadence).
        if elapsed >= 1.0 {
            g.last_kbps = (g.bytes as f64 * 8.0) / elapsed / 1000.0;
            g.have_sample = true;
            g.bytes = 0;
            g.window_start = Instant::now();
        }
    }

    pub fn kbps(&self) -> Option<f64> {
        let g = self.inner.lock();
        if g.have_sample {
            Some(g.last_kbps)
        } else {
            None
        }
    }

    /// Install a BUFFER/BUFFER_LIST probe on `pad`.
    pub fn attach_probe(&self, pad: &gstreamer::Pad) -> Option<gstreamer::PadProbeId> {
        let meter = self.clone();
        pad.add_probe(
            gstreamer::PadProbeType::BUFFER | gstreamer::PadProbeType::BUFFER_LIST,
            move |_pad, info| {
                if let Some(buf) = info.buffer() {
                    meter.note_bytes(buf.size() as u64);
                } else if let Some(list) = info.buffer_list() {
                    for buf in list.iter() {
                        meter.note_bytes(buf.size() as u64);
                    }
                }
                gstreamer::PadProbeReturn::Ok
            },
        )
    }
}

impl Default for BitrateMeter {
    fn default() -> Self {
        Self::new()
    }
}
