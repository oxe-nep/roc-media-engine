//! GStreamer-backed pipelines (in-process, no FFmpeg child processes).

mod bitrate;
mod capture;
mod preview_webrtc;
mod probe;
mod tc_loop;

pub use crate::describe::{
    build_playout_launch, build_spike_tee_launch, build_tc_loop_launch, CaptureLaunchOpts,
    PlayoutLaunchOpts, TcLoopLaunchOpts,
};
pub use preview_webrtc::WebRtcPreview;
pub use probe::probe_gst_devices;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use parking_lot::Mutex;
use roc_config::{ChannelConfig, EncodePreset, PlayoutClientConfig};
use roc_devices::DeviceProbeReport;

use crate::{
    ChannelSnapshot, ChannelStatus, PipelineBackend, PlayoutFileControl, PlayoutSnapshot,
    TcLoopSnapshot, TcLoopSource, TcLoopStatus, WorkflowKind, WorkflowSnapshot,
};

use self::capture::ChannelPipeline;

fn drain_playout_bus_error(pipeline: &gstreamer::Pipeline) -> Option<String> {
    use gstreamer::prelude::*;
    use gstreamer::MessageView;
    let bus = pipeline.bus()?;
    let mut last = None;
    while let Some(msg) = bus.timed_pop(gstreamer::ClockTime::ZERO) {
        if let MessageView::Error(err) = msg.view() {
            last = Some(format!(
                "{}: {}",
                err.error(),
                err.debug().unwrap_or_default()
            ));
        }
    }
    last
}

fn apply_level_peaks(peaks: &mut [f64], s: &gstreamer::StructureRef) {
    let Ok(arr) = s.get::<gstreamer::glib::ValueArray>("peak") else {
        return;
    };
    for (i, val) in arr.iter().enumerate().take(peaks.len()) {
        let Ok(db) = val.get::<f64>() else {
            continue;
        };
        peaks[i] = if db.is_finite() { db.max(-90.0) } else { -90.0 };
    }
}

fn apply_playout_level(peaks: &mut [f64; 2], s: &gstreamer::StructureRef) {
    apply_level_peaks(peaks, s);
}

fn poll_tc_runtime(rt: &mut TcLoopRuntime) {
    use gstreamer::prelude::*;
    use gstreamer::MessageView;
    let Some(pipeline) = rt.pipeline.as_ref() else {
        return;
    };
    let Some(bus) = pipeline.bus() else {
        return;
    };
    while let Some(msg) = bus.timed_pop(gstreamer::ClockTime::ZERO) {
        match msg.view() {
            MessageView::Error(err) => {
                rt.status = TcLoopStatus::Error;
                rt.last_error = Some(format!(
                    "{}: {}",
                    err.error(),
                    err.debug().unwrap_or_default()
                ));
            }
            MessageView::Element(el) => {
                let Some(s) = el.structure() else { continue };
                if s.name() == "level" {
                    apply_level_peaks(&mut rt.audio_peaks, s);
                    rt.last_level_at = Some(std::time::Instant::now());
                }
            }
            _ => {}
        }
    }
    if let Some(at) = rt.last_level_at {
        if at.elapsed() > std::time::Duration::from_millis(500) {
            rt.audio_peaks = [-90.0; 8];
        }
    }
    // Refresh live format label from DeckLink src caps.
    if let Some(fmt) = tc_read_live_format(rt) {
        rt.format_summary = Some(fmt.summary());
    }
}

fn tc_read_live_format(rt: &TcLoopRuntime) -> Option<crate::InputFormat> {
    use gstreamer::prelude::*;
    let pipeline = rt.pipeline.as_ref()?;
    let dl = pipeline.by_name("dlsrc")?;
    let pad = dl.static_pad("src")?;
    let caps = pad.current_caps()?;
    crate::format_from_caps(&caps)
}

fn tc_decklink_signal_present(rt: &TcLoopRuntime) -> Option<bool> {
    use gstreamer::prelude::*;
    let pipeline = rt.pipeline.as_ref()?;
    let dl = pipeline.by_name("dlsrc")?;
    if dl.find_property("signal").is_none() {
        return None;
    }
    Some(dl.property::<bool>("signal"))
}

fn tc_alternate_scan(mode: &str) -> Option<&'static str> {
    match mode {
        "1080p50" => Some("1080i50"),
        "1080i50" => Some("1080p50"),
        "1080p5994" => Some("1080i5994"),
        "1080i5994" => Some("1080p5994"),
        "1080p60" => Some("1080i60"),
        "1080i60" => Some("1080p60"),
        _ => None,
    }
}

/// Decide if TC should relaunch for a new DeckLink mode (format change or p/i flip).
fn tc_adapt_candidate(rt: &TcLoopRuntime) -> Option<String> {
    if !matches!(rt.status, TcLoopStatus::Running | TcLoopStatus::Restarting) {
        return None;
    }
    let cool = |t: Option<std::time::Instant>, secs: u64| {
        t.map(|x| x.elapsed() > std::time::Duration::from_secs(secs))
            .unwrap_or(true)
    };
    // Wrong scan lock with no signal → try alternate a couple of times.
    if tc_decklink_signal_present(rt) == Some(false)
        && rt.signal_flip_attempts < 2
        && cool(rt.last_signal_flip, 6)
        && cool(rt.last_adapt, 6)
    {
        if let Some(alt) = tc_alternate_scan(&rt.locked_mode) {
            return Some(alt.to_string());
        }
    }
    let fmt = tc_read_live_format(rt)?;
    if fmt.width < 1280 || fmt.mode == rt.locked_mode {
        return None;
    }
    if !cool(rt.last_adapt, 3) {
        return None;
    }
    Some(fmt.mode)
}

fn relaunch_tc_for_mode(rt: &mut TcLoopRuntime, channel_id: u32, new_mode: &str) -> Result<()> {
    use gstreamer::prelude::*;
    let was_srt = rt.srt;
    let srt_url = rt.srt_url.clone();
    let flip = tc_alternate_scan(&rt.locked_mode) == Some(new_mode)
        || tc_alternate_scan(new_mode).map(|a| a == rt.locked_mode.as_str()).unwrap_or(false);

    tracing::info!(
        channel_id,
        from = %rt.locked_mode,
        to = %new_mode,
        flip,
        "TC input format adapt — relaunching"
    );

    dispose_tc_webrtc(rt);
    stop_tc_srt(rt, channel_id);
    if let Some(flag) = rt.udp_stop.take() {
        flag.store(true, Ordering::SeqCst);
    }
    if let Some(p) = rt.pipeline.take() {
        let _ = p.set_state(gstreamer::State::Null);
    }

    rt.launch_opts.input_mode = new_mode.to_string();
    rt.launch_opts.output_mode = new_mode.to_string();
    rt.locked_mode = new_mode.to_string();
    rt.format_summary = Some(new_mode.to_string());
    rt.last_adapt = Some(std::time::Instant::now());
    if flip {
        rt.last_signal_flip = Some(std::time::Instant::now());
        rt.signal_flip_attempts = rt.signal_flip_attempts.saturating_add(1);
    } else {
        rt.signal_flip_attempts = 0;
    }
    rt.status = TcLoopStatus::Restarting;

    let launch = build_tc_loop_launch(&rt.launch_opts);
    let pipeline = gstreamer::parse::launch(&launch)
        .with_context(|| format!("parse TC relaunch for channel {channel_id}"))?
        .downcast::<gstreamer::Pipeline>()
        .map_err(|_| anyhow!("TC relaunch did not yield a Pipeline"))?;

    let udp_stop = if matches!(rt.launch_opts.source, TcLoopSource::External) {
        let flag = Arc::new(AtomicBool::new(false));
        tc_loop::spawn_external_tc_updater(&pipeline, rt.launch_opts.udp_port, flag.clone())?;
        Some(flag)
    } else {
        None
    };

    pipeline
        .set_state(gstreamer::State::Playing)
        .context("TC relaunch PLAYING")?;
    let (_res, state, pending) = pipeline.state(gstreamer::ClockTime::from_seconds(3));
    if matches!(state, gstreamer::State::Null | gstreamer::State::Ready)
        && !matches!(pending, gstreamer::State::Playing | gstreamer::State::Paused)
    {
        if let Some(flag) = &udp_stop {
            flag.store(true, Ordering::SeqCst);
        }
        let err = drain_playout_bus_error(&pipeline)
            .unwrap_or_else(|| format!("TC adapt failed to reach PLAYING (state={state:?})"));
        let _ = pipeline.set_state(gstreamer::State::Null);
        rt.status = TcLoopStatus::Error;
        rt.last_error = Some(err.clone());
        bail!("{err}");
    }
    if let Some(err) = drain_playout_bus_error(&pipeline) {
        if let Some(flag) = &udp_stop {
            flag.store(true, Ordering::SeqCst);
        }
        let _ = pipeline.set_state(gstreamer::State::Null);
        rt.status = TcLoopStatus::Error;
        rt.last_error = Some(err.clone());
        bail!("{err}");
    }

    rt.pipeline = Some(pipeline);
    rt.udp_stop = udp_stop;
    rt.status = TcLoopStatus::Running;
    rt.last_error = None;
    rt.aac_pairs = crate::describe::aac_stereo_pairs(rt.launch_opts.preset.audio_channels);

    if was_srt {
        if let Some(url) = srt_url.as_deref() {
            if let Err(e) = start_tc_srt(rt, channel_id, url) {
                tracing::warn!(channel_id, error = %e, "TC SRT re-arm after adapt failed");
            }
        }
    }
    Ok(())
}

fn tc_runtime_snapshot(id: u32, rt: &TcLoopRuntime) -> TcLoopSnapshot {
    let timecode = match (rt.status, rt.source) {
        (TcLoopStatus::Running, TcLoopSource::Tod) => Some(wall_clock_hms()),
        (TcLoopStatus::Running, TcLoopSource::External) => {
            // Best-effort: read current textoverlay text.
            rt.pipeline.as_ref().and_then(|p| {
                use gstreamer::prelude::*;
                let el = p.by_name("tc_text")?;
                let text: String = el.property("text");
                if text.trim().is_empty() || text == "--:--:--" {
                    None
                } else {
                    Some(text)
                }
            })
        }
        _ => None,
    };
    let format = rt
        .format_summary
        .clone()
        .or_else(|| Some(rt.locked_mode.clone()))
        .filter(|s| !s.is_empty());
    TcLoopSnapshot {
        id,
        enabled: rt.enabled,
        status: rt.status,
        source: rt.source,
        udp_port: rt.udp_port,
        fontsize: rt.fontsize,
        opacity: rt.opacity,
        position: rt.position,
        x: rt.x,
        y: rt.y,
        error: rt.last_error.clone(),
        timecode,
        audio_peaks: Some(rt.audio_peaks.to_vec()),
        srt: rt.srt,
        srt_bitrate_kbps: rt.srt_bitrate.as_ref().and_then(|m| m.kbps()),
        format,
        mode: Some(rt.locked_mode.clone()).filter(|s| !s.is_empty()),
    }
}

fn wall_clock_hms() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Local offset is host TZ; for burn-in display we use UTC+local via libc is overkill —
    // clockoverlay itself uses local TZ; API timecode is approximate wall UTC for cards.
    let tod = secs % 86_400;
    let h = tod / 3600;
    let m = (tod % 3600) / 60;
    let s = tod % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

fn start_tc_srt(rt: &mut TcLoopRuntime, channel_id: u32, url: &str) -> Result<()> {
    use gstreamer::prelude::*;
    let pipeline = rt
        .pipeline
        .as_ref()
        .ok_or_else(|| anyhow!("TC not running"))?
        .clone();
    if pipeline.by_name("srt_in").is_none() {
        bail!("TC SRT appsink missing");
    }
    let gst_url = capture::normalize_srt_uri_for_gst(url);
    if rt.srt {
        if let Some(sink) = pipeline.by_name(&format!("srt_sink_{channel_id}")) {
            sink.set_property("uri", &gst_url);
            tracing::info!(channel_id, %gst_url, "updated TC SRT sink URI");
        }
        rt.srt_url = Some(url.to_string());
        return Ok(());
    }
    capture::disarm_srt_appsink_gate(channel_id, &pipeline);
    capture::arm_srt_appsink_gate(channel_id, &pipeline, &gst_url, rt.aac_pairs.max(1))
        .context("TC SRT appsink gate")?;
    let meter = self::bitrate::BitrateMeter::new();
    if let Some(src) = pipeline.by_name(&format!("srt_out_{channel_id}")) {
        if let Some(pad) = src.static_pad("src") {
            let _ = meter.attach_probe(&pad);
        }
    }
    rt.srt_bitrate = Some(meter);
    rt.srt = true;
    rt.srt_url = Some(url.to_string());
    tracing::info!(channel_id, %url, gst_uri = %gst_url, "TC SRT publish attached");
    Ok(())
}

fn stop_tc_srt(rt: &mut TcLoopRuntime, channel_id: u32) {
    if !rt.srt {
        return;
    }
    if let Some(pipeline) = rt.pipeline.as_ref() {
        capture::disarm_srt_appsink_gate(channel_id, pipeline);
    }
    rt.srt = false;
    rt.srt_bitrate = None;
    tracing::info!(channel_id, "TC SRT publish detached");
}

fn start_tc_webrtc(
    rt: &mut TcLoopRuntime,
    channel_id: u32,
    pair: u8,
    signal_tx: crate::PreviewSignalTx,
) -> Result<String> {
    let pipeline = rt
        .pipeline
        .as_ref()
        .ok_or_else(|| anyhow!("TC not running"))?
        .clone();
    // Fresh webrtcbin each open — reuse after park sticks ICE with a new browser PC.
    if let Some(old) = rt.webrtc_preview.take() {
        old.dispose(&pipeline);
    }
    let preview =
        preview_webrtc::WebRtcPreview::attach(&pipeline, channel_id, pair, signal_tx)?;
    let sid = preview.session_id.clone();
    rt.webrtc_preview = Some(preview);
    Ok(sid)
}

fn stop_tc_webrtc(rt: &mut TcLoopRuntime) {
    let Some(preview) = rt.webrtc_preview.as_mut() else {
        return;
    };
    if let Some(pipeline) = rt.pipeline.as_ref() {
        preview.park(pipeline);
    }
}

fn dispose_tc_webrtc(rt: &mut TcLoopRuntime) {
    let Some(preview) = rt.webrtc_preview.take() else {
        return;
    };
    if let Some(pipeline) = rt.pipeline.as_ref() {
        preview.dispose(pipeline);
    }
}

fn playout_seek_pipeline(
    pipeline: &gstreamer::Pipeline,
    position_sec: f64,
    accurate: bool,
) -> Result<()> {
    use gstreamer::prelude::*;
    let ns = if position_sec.is_finite() && position_sec > 0.0 {
        (position_sec * 1_000_000_000.0).round() as u64
    } else {
        0
    };
    let mut flags = gstreamer::SeekFlags::FLUSH;
    if accurate {
        flags.insert(gstreamer::SeekFlags::ACCURATE);
    } else {
        flags.insert(gstreamer::SeekFlags::KEY_UNIT);
    }
    pipeline
        .seek_simple(flags, gstreamer::ClockTime::from_nseconds(ns))
        .map_err(|e| anyhow!("seek to {position_sec:.3}s failed: {e}"))?;
    // Brief settle only — long waits block the playout lock and freeze scrub.
    let wait_ms = if accurate { 80 } else { 0 };
    if wait_ms > 0 {
        let _ = pipeline.state(gstreamer::ClockTime::from_mseconds(wait_ms));
    }
    Ok(())
}

fn element_time_sec(el: &gstreamer::Element) -> (Option<f64>, Option<f64>) {
    use gstreamer::prelude::*;
    let position_sec = el
        .query_position::<gstreamer::ClockTime>()
        .map(|t| t.nseconds() as f64 / 1_000_000_000.0)
        .filter(|p| p.is_finite() && *p >= 0.0);
    let duration_sec = el
        .query_duration::<gstreamer::ClockTime>()
        .map(|t| t.nseconds() as f64 / 1_000_000_000.0)
        .filter(|d| d.is_finite() && *d > 0.0);
    (position_sec, duration_sec)
}

/// File media time from decoder-side `vpos` (PTS), then demux `d`, then pipeline.
/// Avoids DeckLink sink clocks that stick at EOF while frames are still held.
fn query_playout_media_time(pipeline: &gstreamer::Pipeline) -> (Option<f64>, Option<f64>) {
    use gstreamer::prelude::*;
    let mut position_sec = None;
    let mut duration_sec = None;
    for name in ["vpos", "d"] {
        if let Some(el) = pipeline.by_name(name) {
            let (p, d) = element_time_sec(&el);
            position_sec = position_sec.or(p);
            duration_sec = duration_sec.or(d);
        }
    }
    if position_sec.is_none() || duration_sec.is_none() {
        let (p, d) = element_time_sec(pipeline.upcast_ref());
        position_sec = position_sec.or(p);
        duration_sec = duration_sec.or(d);
    }
    (position_sec, duration_sec)
}

fn poll_playout_runtime(p: &mut PlayoutRuntime) {
    use gstreamer::prelude::*;
    use gstreamer::MessageView;
    let Some(pipeline) = p.pipeline.clone() else {
        return;
    };
    let Some(bus) = pipeline.bus() else {
        return;
    };
    let mut seek_to_in = false;
    let mut hit_eos = false;
    while let Some(msg) = bus.timed_pop(gstreamer::ClockTime::ZERO) {
        match msg.view() {
            MessageView::Error(err) => {
                let text = format!(
                    "{}: {}",
                    err.error(),
                    err.debug().unwrap_or_default()
                );
                tracing::error!(client = %p.name, error = %text, "playout bus error");
                p.status = ChannelStatus::Error;
                p.last_error = Some(text);
            }
            MessageView::Eos(_) => {
                tracing::info!(client = %p.name, "playout EOS");
                hit_eos = true;
            }
            MessageView::Element(el) => {
                if let Some(s) = el.structure() {
                    if s.name() == "level" {
                        apply_playout_level(&mut p.audio_peaks, s);
                        p.last_level_at = Some(std::time::Instant::now());
                        if matches!(p.status, ChannelStatus::Waiting) {
                            p.status = ChannelStatus::Running;
                        }
                    }
                }
            }
            MessageView::StateChanged(sc) => {
                if sc.current() == gstreamer::State::Playing
                    && matches!(p.status, ChannelStatus::Waiting)
                {
                    p.status = ChannelStatus::Running;
                }
            }
            _ => {}
        }
    }

    if matches!(
        p.status,
        ChannelStatus::Running | ChannelStatus::Paused | ChannelStatus::Waiting
    ) {
        let (gst_pos, dur) = if p.is_file {
            query_playout_media_time(&pipeline)
        } else {
            element_time_sec(pipeline.upcast_ref())
        };
        if let Some(d) = dur {
            p.duration_sec = Some(d);
        }
        // While paused, keep scrubbed/seek position sticky — GST/DeckLink often
        // still reports EOF until the next Playing preroll.
        if !matches!(p.status, ChannelStatus::Paused) {
            if let Some(pos) = gst_pos {
                p.position_sec = Some(pos);
                if p.is_file && matches!(p.status, ChannelStatus::Running) {
                    if let Some(out) = p.mark_out_sec {
                        if out > p.mark_in_sec && pos >= out - 0.04 {
                            if p.loop_file {
                                seek_to_in = true;
                            } else if let Err(e) = pipeline.set_state(gstreamer::State::Paused) {
                                tracing::warn!(client = %p.name, error = %e, "pause at mark-out failed");
                            } else {
                                p.status = ChannelStatus::Paused;
                                p.position_sec = Some(out);
                            }
                        }
                    }
                }
            }
        }
    }

    if hit_eos {
        if p.is_file && p.loop_file && matches!(p.status, ChannelStatus::Running) {
            seek_to_in = true;
        } else if matches!(p.status, ChannelStatus::Running) {
            // Park paused at EOF so the user can scrub / set marks.
            if pipeline.set_state(gstreamer::State::Paused).is_ok() {
                p.status = ChannelStatus::Paused;
                p.position_sec = p.duration_sec.or(p.position_sec);
            } else {
                p.status = ChannelStatus::Stopped;
                p.position_sec = p.duration_sec.or(p.position_sec);
            }
        }
        // Ignore EOS while already paused (stale bus after scrub/seek).
    }

    if seek_to_in {
        let target = p.mark_in_sec.max(0.0);
        match playout_seek_pipeline(&pipeline, target, false) {
            Ok(()) => {
                p.position_sec = Some(target);
                if !matches!(p.status, ChannelStatus::Running) {
                    if pipeline.set_state(gstreamer::State::Playing).is_ok() {
                        p.status = ChannelStatus::Running;
                    }
                }
            }
            Err(e) => {
                tracing::warn!(client = %p.name, error = %e, "loop seek failed");
                p.status = ChannelStatus::Stopped;
            }
        }
    }

    if let Some(at) = p.last_level_at {
        if at.elapsed() > std::time::Duration::from_millis(500) {
            p.audio_peaks = [-90.0; 2];
        }
    }
}

pub struct GstBackend {
    max_nvenc: usize,
    nvenc_used: AtomicUsize,
    /// Serialize all graph mutations (attach/detach/start/stop) across channels.
    /// Concurrent multi-channel REC stop previously raced inside GStreamer/DeckLink.
    gst_op: Mutex<()>,
    channels: Mutex<HashMap<u32, ChannelPipeline>>,
    presets: Mutex<HashMap<u32, EncodePreset>>,
    configs: Mutex<HashMap<u32, ChannelConfig>>,
    playout: Mutex<HashMap<String, PlayoutRuntime>>,
    tc_loops: Mutex<HashMap<u32, TcLoopRuntime>>,
    workflows: Mutex<HashMap<u32, (WorkflowKind, bool)>>,
}

struct PlayoutRuntime {
    name: String,
    device: String,
    status: ChannelStatus,
    source: Option<String>,
    format_code: Option<String>,
    last_error: Option<String>,
    audio_peaks: [f64; 2],
    last_level_at: Option<std::time::Instant>,
    pipeline: Option<gstreamer::Pipeline>,
    is_file: bool,
    loop_file: bool,
    mark_in_sec: f64,
    mark_out_sec: Option<f64>,
    position_sec: Option<f64>,
    duration_sec: Option<f64>,
}

struct TcLoopRuntime {
    enabled: bool,
    status: TcLoopStatus,
    source: TcLoopSource,
    udp_port: u16,
    fontsize: u32,
    opacity: f64,
    position: crate::TcLoopPosition,
    x: f64,
    y: f64,
    last_error: Option<String>,
    audio_peaks: [f64; 8],
    last_level_at: Option<std::time::Instant>,
    pipeline: Option<gstreamer::Pipeline>,
    udp_stop: Option<Arc<AtomicBool>>,
    /// AAC stereo pairs in MPEG-TS (for SRT PMT gate).
    aac_pairs: usize,
    srt: bool,
    srt_url: Option<String>,
    srt_bitrate: Option<self::bitrate::BitrateMeter>,
    webrtc_preview: Option<preview_webrtc::WebRtcPreview>,
    /// Opts used to (re)launch — updated when format adapts.
    launch_opts: TcLoopLaunchOpts,
    locked_mode: String,
    format_summary: Option<String>,
    last_adapt: Option<std::time::Instant>,
    last_signal_flip: Option<std::time::Instant>,
    signal_flip_attempts: u8,
}

impl GstBackend {
    pub fn new() -> Result<Self> {
        gstreamer::init().context("gstreamer::init")?;
        Ok(Self {
            max_nvenc: 8,
            nvenc_used: AtomicUsize::new(0),
            gst_op: Mutex::new(()),
            channels: Mutex::new(HashMap::new()),
            presets: Mutex::new(HashMap::new()),
            configs: Mutex::new(HashMap::new()),
            playout: Mutex::new(HashMap::new()),
            tc_loops: Mutex::new(HashMap::new()),
            workflows: Mutex::new(HashMap::new()),
        })
    }

    pub fn with_nvenc_limit(mut self, limit: usize) -> Self {
        self.max_nvenc = limit;
        self
    }
}

impl PipelineBackend for GstBackend {
    fn probe_devices(&self) -> Result<DeviceProbeReport> {
        probe_gst_devices()
    }

    fn ensure_channel(
        &self,
        ch: &ChannelConfig,
        encode: &EncodePreset,
        record: &EncodePreset,
    ) -> Result<()> {
        self.configs.lock().insert(ch.id, ch.clone());
        self.presets.lock().insert(ch.id, encode.clone());
        let mut map = self.channels.lock();
        if !map.contains_key(&ch.id) {
            map.insert(ch.id, ChannelPipeline::new(ch, encode, record)?);
        } else if let Some(p) = map.get_mut(&ch.id) {
            p.update_config(ch, encode, record);
        }
        Ok(())
    }

    fn apply_encode_preset(
        &self,
        channel_id: u32,
        preset_id: &str,
        preset: &EncodePreset,
    ) -> Result<()> {
        let _gst = self.gst_op.lock();
        self.presets.lock().insert(channel_id, preset.clone());
        if let Some(ch) = self.configs.lock().get_mut(&channel_id) {
            ch.encode_preset = Some(preset_id.to_string());
        }
        let mut map = self.channels.lock();
        let pipe = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        pipe.apply_encode_preset(preset_id, preset)
    }

    fn apply_record_preset(
        &self,
        channel_id: u32,
        preset_id: &str,
        preset: &EncodePreset,
    ) -> Result<()> {
        let _gst = self.gst_op.lock();
        if let Some(ch) = self.configs.lock().get_mut(&channel_id) {
            ch.record_preset = Some(preset_id.to_string());
        }
        let mut map = self.channels.lock();
        let pipe = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        pipe.apply_record_preset(preset_id, preset)
    }

    fn start_capture(&self, channel_id: u32) -> Result<()> {
        let _gst = self.gst_op.lock();
        let used = self.nvenc_used.load(Ordering::SeqCst);
        if used >= self.max_nvenc {
            bail!("NVENC session limit reached ({}/{})", used, self.max_nvenc);
        }
        let mut map = self.channels.lock();
        let pipe = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        if matches!(pipe.status, ChannelStatus::Running | ChannelStatus::Waiting) {
            return Ok(());
        }
        pipe.start()?;
        self.nvenc_used.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn stop_capture(&self, channel_id: u32) -> Result<()> {
        // Finalize REC EOS outside gst_op (same pattern as stop_*_recording).
        let (pending_proxy, pending_hq, was_live) = {
            let _gst = self.gst_op.lock();
            let mut map = self.channels.lock();
            let pipe = map
                .get_mut(&channel_id)
                .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
            let was_live =
                matches!(pipe.status, ChannelStatus::Running | ChannelStatus::Waiting);
            let pending_proxy = pipe.begin_stop_proxy_recording()?;
            let pending_hq = pipe.begin_stop_hq_recording()?;
            (pending_proxy, pending_hq, was_live)
        };
        for pending in [pending_proxy, pending_hq].into_iter().flatten() {
            pending.wait_eos();
            let _gst = self.gst_op.lock();
            pending.finish();
        }
        {
            let _gst = self.gst_op.lock();
            let mut map = self.channels.lock();
            let pipe = map
                .get_mut(&channel_id)
                .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
            pipe.stop()?;
            if was_live {
                let prev = self.nvenc_used.fetch_sub(1, Ordering::SeqCst);
                if prev == 0 {
                    self.nvenc_used.store(0, Ordering::SeqCst);
                }
            }
        }
        Ok(())
    }

    fn start_recording(&self, channel_id: u32, path: &str) -> Result<()> {
        self.start_hq_recording(channel_id, path)
    }

    fn stop_recording(&self, channel_id: u32) -> Result<()> {
        self.stop_hq_recording(channel_id)
    }

    fn start_proxy_recording(&self, channel_id: u32, path: &str) -> Result<()> {
        let _gst = self.gst_op.lock();
        let mut map = self.channels.lock();
        let pipe = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        pipe.start_proxy_recording(path)
    }

    fn stop_proxy_recording(&self, channel_id: u32) -> Result<()> {
        let pending = {
            let _gst = self.gst_op.lock();
            let mut map = self.channels.lock();
            let pipe = map
                .get_mut(&channel_id)
                .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
            pipe.begin_stop_proxy_recording()?
        };
        // ProRes / qtmux drain can take seconds — never hold gst_op across wait.
        if let Some(pending) = pending {
            pending.wait_eos();
            let _gst = self.gst_op.lock();
            pending.finish();
        }
        Ok(())
    }

    fn start_hq_recording(&self, channel_id: u32, path: &str) -> Result<()> {
        let _gst = self.gst_op.lock();
        let mut map = self.channels.lock();
        let pipe = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        pipe.start_hq_recording(path)
    }

    fn stop_hq_recording(&self, channel_id: u32) -> Result<()> {
        let pending = {
            let _gst = self.gst_op.lock();
            let mut map = self.channels.lock();
            let pipe = map
                .get_mut(&channel_id)
                .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
            pipe.begin_stop_hq_recording()?
        };
        // ProRes / qtmux drain can take seconds — never hold gst_op across wait.
        if let Some(pending) = pending {
            pending.wait_eos();
            let _gst = self.gst_op.lock();
            pending.finish();
        }
        Ok(())
    }

    fn start_srt(&self, channel_id: u32, url: &str) -> Result<()> {
        let _gst = self.gst_op.lock();
        // TC burn-in owns SRT when its proxy encode pipeline is running.
        {
            let mut tc = self.tc_loops.lock();
            if let Some(rt) = tc.get_mut(&channel_id) {
                if matches!(
                    rt.status,
                    TcLoopStatus::Running | TcLoopStatus::Restarting
                ) {
                    return start_tc_srt(rt, channel_id, url);
                }
            }
        }
        let mut map = self.channels.lock();
        let pipe = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        pipe.start_srt(url)
    }

    fn stop_srt(&self, channel_id: u32) -> Result<()> {
        let _gst = self.gst_op.lock();
        {
            let mut tc = self.tc_loops.lock();
            if let Some(rt) = tc.get_mut(&channel_id) {
                if rt.srt
                    || matches!(
                        rt.status,
                        TcLoopStatus::Running | TcLoopStatus::Restarting
                    )
                {
                    stop_tc_srt(rt, channel_id);
                    return Ok(());
                }
            }
        }
        let mut map = self.channels.lock();
        let pipe = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        pipe.stop_srt()
    }

    fn start_webrtc_preview(
        &self,
        channel_id: u32,
        pair: u8,
        signal_tx: crate::PreviewSignalTx,
    ) -> Result<String> {
        let _gst = self.gst_op.lock();
        // Only one preview session engine-wide.
        {
            let mut map = self.channels.lock();
            for (id, pipe) in map.iter_mut() {
                if *id != channel_id {
                    pipe.stop_webrtc_preview();
                }
            }
        }
        {
            let mut tc = self.tc_loops.lock();
            for (id, rt) in tc.iter_mut() {
                if *id != channel_id {
                    stop_tc_webrtc(rt);
                }
            }
            if let Some(rt) = tc.get_mut(&channel_id) {
                if matches!(
                    rt.status,
                    TcLoopStatus::Running | TcLoopStatus::Restarting
                ) {
                    return start_tc_webrtc(rt, channel_id, pair, signal_tx);
                }
            }
        }
        let mut map = self.channels.lock();
        let pipe = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        pipe.start_webrtc_preview(pair, signal_tx)
    }

    fn set_webrtc_answer(&self, channel_id: u32, sdp: &str) -> Result<()> {
        let _gst = self.gst_op.lock();
        {
            let map = self.tc_loops.lock();
            if let Some(rt) = map.get(&channel_id) {
                if let Some(p) = rt.webrtc_preview.as_ref() {
                    return p.set_remote_answer(sdp);
                }
            }
        }
        let map = self.channels.lock();
        let pipe = map
            .get(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        pipe.set_webrtc_answer(sdp)
    }

    fn add_webrtc_ice(&self, channel_id: u32, sdp_mline_index: u32, candidate: &str) -> Result<()> {
        let _gst = self.gst_op.lock();
        {
            let map = self.tc_loops.lock();
            if let Some(rt) = map.get(&channel_id) {
                if let Some(p) = rt.webrtc_preview.as_ref() {
                    p.add_ice_candidate(sdp_mline_index, candidate);
                    return Ok(());
                }
            }
        }
        let map = self.channels.lock();
        let pipe = map
            .get(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        pipe.add_webrtc_ice(sdp_mline_index, candidate)
    }

    fn stop_webrtc_preview(&self, channel_id: u32) -> Result<()> {
        let _gst = self.gst_op.lock();
        {
            let mut tc = self.tc_loops.lock();
            if let Some(rt) = tc.get_mut(&channel_id) {
                stop_tc_webrtc(rt);
            }
        }
        let mut map = self.channels.lock();
        if let Some(pipe) = map.get_mut(&channel_id) {
            pipe.stop_webrtc_preview();
        }
        Ok(())
    }

    fn stop_webrtc_preview_session(&self, channel_id: u32, session_id: &str) -> Result<()> {
        let _gst = self.gst_op.lock();
        {
            let mut tc = self.tc_loops.lock();
            if let Some(rt) = tc.get_mut(&channel_id) {
                if let Some(p) = rt.webrtc_preview.as_ref() {
                    if p.session_id == session_id {
                        stop_tc_webrtc(rt);
                        return Ok(());
                    }
                }
            }
        }
        let mut map = self.channels.lock();
        if let Some(pipe) = map.get_mut(&channel_id) {
            pipe.stop_webrtc_preview_session(session_id);
        }
        Ok(())
    }

    fn channel_snapshot(&self, channel_id: u32) -> Result<ChannelSnapshot> {
        // Same lock order as mutations: gst_op → channels (poll_bus may adapt/relaunch).
        let _gst = self.gst_op.lock();
        let mut map = self.channels.lock();
        let pipe = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        pipe.poll_bus();
        Ok(pipe.snapshot(self.nvenc_used.load(Ordering::SeqCst)))
    }

    fn list_channels(&self) -> Vec<ChannelSnapshot> {
        let _gst = self.gst_op.lock();
        let mut map = self.channels.lock();
        let used = self.nvenc_used.load(Ordering::SeqCst);
        let mut ids: Vec<u32> = map.keys().copied().collect();
        ids.sort_unstable();
        ids.into_iter()
            .filter_map(|id| {
                let p = map.get_mut(&id)?;
                p.poll_bus();
                Some(p.snapshot(used))
            })
            .collect()
    }

    fn list_channel_meters(&self) -> Vec<ChannelSnapshot> {
        // Meter tick: peaks/status only — never adapt/relaunch under gst_op.
        let _gst = self.gst_op.lock();
        let mut map = self.channels.lock();
        let used = self.nvenc_used.load(Ordering::SeqCst);
        let mut ids: Vec<u32> = map.keys().copied().collect();
        ids.sort_unstable();
        ids.into_iter()
            .filter_map(|id| {
                let p = map.get_mut(&id)?;
                p.poll_bus_light();
                Some(p.snapshot(used))
            })
            .collect()
    }

    fn nvenc_used(&self) -> usize {
        self.nvenc_used.load(Ordering::SeqCst)
    }

    fn nvenc_limit(&self) -> usize {
        self.max_nvenc
    }

    fn start_playout(&self, client: &PlayoutClientConfig, source: &str) -> Result<()> {
        use gstreamer::prelude::*;
        use crate::resolve_playout_format_code;

        let format_code = resolve_playout_format_code(client.format_code.as_deref(), source);
        let num_id = client
            .id
            .strip_prefix("decode-")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        let hls_base = std::env::var("ROC_MEDIA_HLS_DIR").unwrap_or_else(|_| {
            "/opt/applications/roc-media-engine/hls".into()
        });
        let hls_dir = if num_id > 0 {
            let dir = format!("{hls_base}/playout/{num_id}");
            let _ = std::fs::create_dir_all(&dir);
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for ent in rd.flatten() {
                    let name = ent.file_name();
                    let n = name.to_string_lossy();
                    if n.ends_with(".m3u8")
                        || n.ends_with(".ts")
                        || n.starts_with("thumb")
                        || n.starts_with("pv")
                    {
                        let _ = std::fs::remove_file(ent.path());
                    }
                }
            }
            Some(dir)
        } else {
            None
        };
        let audio = crate::probe_playout_audio(source);
        tracing::info!(
            client_id = %client.id,
            audio_pairs = audio.pairs,
            audio_compressed = audio.compressed,
            "playout audio probe"
        );
        let launch = build_playout_launch(&PlayoutLaunchOpts {
            source: source.to_string(),
            device: client.device.clone(),
            format_code: format_code.clone(),
            hls_dir,
            audio_pairs: audio.pairs,
            audio_compressed: audio.compressed,
        });
        tracing::info!(%launch, client_id = %client.id, %format_code, "starting playout pipeline");
        let pipeline = gstreamer::parse::launch(&launch)
            .with_context(|| format!("parse playout launch for {}", client.id))?
            .downcast::<gstreamer::Pipeline>()
            .map_err(|_| anyhow!("playout launch did not yield a Pipeline"))?;
        pipeline
            .set_state(gstreamer::State::Playing)
            .context("playout PLAYING")?;
        // Surface immediate DeckLink / negotiation failures instead of a silent "running".
        let (_res, state, pending) = pipeline
            .state(gstreamer::ClockTime::from_seconds(3));
        if matches!(state, gstreamer::State::Null | gstreamer::State::Ready)
            && !matches!(pending, gstreamer::State::Playing | gstreamer::State::Paused)
        {
            let err = drain_playout_bus_error(&pipeline)
                .unwrap_or_else(|| format!("playout failed to reach PLAYING (state={state:?})"));
            let _ = pipeline.set_state(gstreamer::State::Null);
            bail!("{err}");
        }
        if let Some(err) = drain_playout_bus_error(&pipeline) {
            let _ = pipeline.set_state(gstreamer::State::Null);
            bail!("{err}");
        }

        let initial = if source.contains("mode=listener") {
            ChannelStatus::Waiting
        } else {
            ChannelStatus::Running
        };

        let is_file = !source.starts_with("srt://");
        let mut map = self.playout.lock();
        if let Some(old) = map.get_mut(&client.id) {
            if let Some(p) = old.pipeline.take() {
                let _ = p.set_state(gstreamer::State::Null);
            }
        }
        let (_, duration_sec) = query_playout_media_time(&pipeline);
        map.insert(
            client.id.clone(),
            PlayoutRuntime {
                name: client.name.clone(),
                device: client.device.clone(),
                status: initial,
                source: Some(source.to_string()),
                format_code: Some(format_code),
                last_error: None,
                audio_peaks: [-90.0; 2],
                last_level_at: None,
                pipeline: Some(pipeline),
                is_file,
                loop_file: false,
                mark_in_sec: 0.0,
                mark_out_sec: None,
                position_sec: if is_file { Some(0.0) } else { None },
                duration_sec,
            },
        );
        Ok(())
    }

    fn stop_playout(&self, client_id: &str) -> Result<()> {
        use gstreamer::prelude::*;
        let mut map = self.playout.lock();
        if let Some(p) = map.get_mut(client_id) {
            if let Some(pipe) = p.pipeline.take() {
                let _ = pipe.set_state(gstreamer::State::Null);
            }
            p.status = ChannelStatus::Stopped;
            p.source = None;
            p.format_code = None;
            p.position_sec = None;
        }
        Ok(())
    }

    fn pause_playout(&self, client_id: &str) -> Result<()> {
        use gstreamer::prelude::*;
        let mut map = self.playout.lock();
        let p = map
            .get_mut(client_id)
            .with_context(|| format!("playout {client_id} not running"))?;
        let pipe = p
            .pipeline
            .as_ref()
            .with_context(|| format!("playout {client_id} has no pipeline"))?
            .clone();
        if p.is_file {
            let (pos, dur) = query_playout_media_time(&pipe);
            if let Some(d) = dur {
                p.duration_sec = Some(d);
            }
            if let Some(pos) = pos {
                p.position_sec = Some(pos);
            }
        }
        pipe.set_state(gstreamer::State::Paused)
            .context("playout PAUSED")?;
        p.status = ChannelStatus::Paused;
        Ok(())
    }

    fn resume_playout(&self, client_id: &str) -> Result<()> {
        use gstreamer::prelude::*;
        let mut map = self.playout.lock();
        let p = map
            .get_mut(client_id)
            .with_context(|| format!("playout {client_id} not running"))?;
        let pipe = p
            .pipeline
            .as_ref()
            .with_context(|| format!("playout {client_id} has no pipeline"))?
            .clone();
        // If parked at mark-out / EOF, restart from mark-in on resume.
        if p.is_file {
            let pos = p.position_sec.unwrap_or(0.0);
            let at_out = p
                .mark_out_sec
                .filter(|o| *o > p.mark_in_sec)
                .is_some_and(|o| pos >= o - 0.05);
            let at_eof = p
                .duration_sec
                .is_some_and(|d| d > 0.0 && pos >= d - 0.05);
            let target = if at_out || at_eof {
                p.mark_in_sec.max(0.0)
            } else {
                pos
            };
            if (target - pos).abs() > 0.02 || at_out || at_eof {
                playout_seek_pipeline(&pipe, target, false)?;
            }
            p.position_sec = Some(target);
        }
        // Wait for scrub preroll to settle before PLAYING (avoids DeckLink state races).
        let _ = pipe.state(gstreamer::ClockTime::from_mseconds(250));
        pipe.set_state(gstreamer::State::Playing)
            .context("playout PLAYING")?;
        let (_res, state, pending) = pipe.state(gstreamer::ClockTime::from_seconds(2));
        if !matches!(state, gstreamer::State::Playing)
            && !matches!(pending, gstreamer::State::Playing)
        {
            bail!("playout PLAYING: Element failed to change its state (state={state:?})");
        }
        p.status = ChannelStatus::Running;
        p.last_error = None;
        Ok(())
    }

    fn seek_playout(&self, client_id: &str, position_sec: f64) -> Result<()> {
        use gstreamer::prelude::*;
        let mut map = self.playout.lock();
        let p = map
            .get_mut(client_id)
            .with_context(|| format!("playout {client_id} not running"))?;
        if !p.is_file {
            bail!("seek is only supported for file playout");
        }
        let pipe = p
            .pipeline
            .as_ref()
            .with_context(|| format!("playout {client_id} has no pipeline"))?
            .clone();
        let target = {
            let mut t = if position_sec.is_finite() {
                position_sec.max(0.0)
            } else {
                0.0
            };
            if let Some(d) = p.duration_sec {
                t = t.min(d.max(0.0));
            }
            t
        };
        let paused = matches!(p.status, ChannelStatus::Paused);
        // Accurate seek while paused so in/out scrub lands on the shown frame.
        playout_seek_pipeline(&pipe, target, paused)?;
        // Drop stale EOS from the previous play-through so poll does not park at EOF.
        if let Some(bus) = pipe.bus() {
            while let Some(msg) = bus.timed_pop(gstreamer::ClockTime::ZERO) {
                let _ = msg;
            }
        }
        p.position_sec = Some(target);
        // Already paused — do not re-enter PAUSED (DeckLink state races / scrub freeze).
        Ok(())
    }

    fn set_playout_file_control(&self, client_id: &str, control: &PlayoutFileControl) -> Result<()> {
        use gstreamer::prelude::*;
        let mut map = self.playout.lock();
        let Some(p) = map.get_mut(client_id) else {
            return Ok(());
        };
        if !p.is_file {
            return Ok(());
        }
        p.loop_file = control.loop_file;
        p.mark_in_sec = control.mark_in_sec.max(0.0);
        p.mark_out_sec = control
            .mark_out_sec
            .filter(|o| o.is_finite() && *o > p.mark_in_sec);
        // If already past new mark-out while running, park there.
        if let (Some(pos), Some(out)) = (p.position_sec, p.mark_out_sec) {
            if matches!(p.status, ChannelStatus::Running) && pos >= out {
                if let Some(pipe) = p.pipeline.as_ref() {
                    let _ = playout_seek_pipeline(pipe, out, true);
                    let _ = pipe.set_state(gstreamer::State::Paused);
                    p.status = ChannelStatus::Paused;
                    p.position_sec = Some(out);
                }
            }
        }
        Ok(())
    }

    fn list_playout(&self) -> Vec<PlayoutSnapshot> {
        let mut map = self.playout.lock();
        for p in map.values_mut() {
            poll_playout_runtime(p);
        }
        let mut out: Vec<_> = map
            .iter()
            .map(|(id, p)| PlayoutSnapshot {
                id: id.clone(),
                name: p.name.clone(),
                status: p.status,
                device: p.device.clone(),
                source: p.source.clone(),
                format_code: p.format_code.clone(),
                last_error: p.last_error.clone(),
                audio_peaks: Some(p.audio_peaks.to_vec()),
                position_sec: p.position_sec,
                duration_sec: p.duration_sec,
            })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    fn start_tc_loop(&self, channel_id: u32, opts: &TcLoopLaunchOpts) -> Result<()> {
        use gstreamer::prelude::*;
        let _gst = self.gst_op.lock();

        // Tear down any previous TC on this channel.
        {
            let mut map = self.tc_loops.lock();
            if let Some(mut old) = map.remove(&channel_id) {
                dispose_tc_webrtc(&mut old);
                stop_tc_srt(&mut old, channel_id);
                if let Some(flag) = &old.udp_stop {
                    flag.store(true, Ordering::SeqCst);
                }
                if let Some(p) = old.pipeline {
                    let _ = p.set_state(gstreamer::State::Null);
                }
                let _ = self.nvenc_used.fetch_sub(1, Ordering::SeqCst);
            }
        }

        let launch = build_tc_loop_launch(opts);
        tracing::info!(channel_id, %launch, "starting TC burn-in pipeline");
        let pipeline = gstreamer::parse::launch(&launch)
            .with_context(|| format!("parse TC launch for channel {channel_id}"))?
            .downcast::<gstreamer::Pipeline>()
            .map_err(|_| anyhow!("TC launch did not yield a Pipeline"))?;

        let udp_stop = if matches!(opts.source, TcLoopSource::External) {
            let flag = Arc::new(AtomicBool::new(false));
            tc_loop::spawn_external_tc_updater(&pipeline, opts.udp_port, flag.clone())?;
            Some(flag)
        } else {
            None
        };

        pipeline
            .set_state(gstreamer::State::Playing)
            .context("TC PLAYING")?;
        let (_res, state, pending) = pipeline.state(gstreamer::ClockTime::from_seconds(3));
        if matches!(state, gstreamer::State::Null | gstreamer::State::Ready)
            && !matches!(pending, gstreamer::State::Playing | gstreamer::State::Paused)
        {
            if let Some(flag) = &udp_stop {
                flag.store(true, Ordering::SeqCst);
            }
            let err = drain_playout_bus_error(&pipeline)
                .unwrap_or_else(|| format!("TC failed to reach PLAYING (state={state:?})"));
            let _ = pipeline.set_state(gstreamer::State::Null);
            bail!("{err}");
        }
        if let Some(err) = drain_playout_bus_error(&pipeline) {
            if let Some(flag) = &udp_stop {
                flag.store(true, Ordering::SeqCst);
            }
            let _ = pipeline.set_state(gstreamer::State::Null);
            bail!("{err}");
        }

        let aac_pairs = crate::describe::aac_stereo_pairs(opts.preset.audio_channels);
        self.nvenc_used.fetch_add(1, Ordering::SeqCst);
        self.tc_loops.lock().insert(
            channel_id,
            TcLoopRuntime {
                enabled: true,
                status: TcLoopStatus::Running,
                source: opts.source,
                udp_port: opts.udp_port,
                fontsize: opts.fontsize,
                opacity: opts.opacity,
                position: opts.position,
                x: opts.x,
                y: opts.y,
                last_error: None,
                audio_peaks: [-90.0; 8],
                last_level_at: None,
                pipeline: Some(pipeline),
                udp_stop,
                aac_pairs,
                srt: false,
                srt_url: None,
                srt_bitrate: None,
                webrtc_preview: None,
                launch_opts: opts.clone(),
                locked_mode: opts.input_mode.clone(),
                format_summary: Some(opts.input_mode.clone()),
                last_adapt: None,
                last_signal_flip: None,
                signal_flip_attempts: 0,
            },
        );
        self.workflows
            .lock()
            .insert(channel_id, (WorkflowKind::Timecode, true));
        Ok(())
    }

    fn stop_tc_loop(&self, channel_id: u32) -> Result<()> {
        use gstreamer::prelude::*;
        let _gst = self.gst_op.lock();
        let mut map = self.tc_loops.lock();
        if let Some(mut rt) = map.remove(&channel_id) {
            dispose_tc_webrtc(&mut rt);
            stop_tc_srt(&mut rt, channel_id);
            if let Some(flag) = rt.udp_stop.take() {
                flag.store(true, Ordering::SeqCst);
            }
            if let Some(p) = rt.pipeline.take() {
                let _ = p.set_state(gstreamer::State::Null);
            }
            let _ = self.nvenc_used.fetch_sub(1, Ordering::SeqCst);
        }
        if let Some(entry) = self.workflows.lock().get_mut(&channel_id) {
            if matches!(entry.0, WorkflowKind::Timecode) {
                entry.1 = false;
            }
        }
        Ok(())
    }

    fn update_tc_overlay(
        &self,
        channel_id: u32,
        fontsize: u32,
        opacity: f64,
        x: f64,
        y: f64,
        position: crate::TcLoopPosition,
    ) -> Result<()> {
        use gstreamer::prelude::*;
        let _gst = self.gst_op.lock();
        let mut map = self.tc_loops.lock();
        let rt = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("TC not running on channel {channel_id}"))?;
        let pipeline = rt
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow!("TC pipeline missing"))?;
        let el = pipeline
            .by_name("tc_overlay")
            .or_else(|| pipeline.by_name("tc_text"))
            .ok_or_else(|| anyhow!("TC overlay element missing"))?;

        let fontsize = fontsize.clamp(12, 200);
        let opacity = opacity.clamp(0.15, 1.0);
        let x = x.clamp(0.0, 1.0);
        let y = y.clamp(0.0, 1.0);
        let font_desc = format!("Sans Bold {fontsize}px");
        let color = crate::describe::tc_overlay_color(opacity);

        el.set_property("font-desc", &font_desc);
        el.set_property("xpos", x);
        el.set_property("ypos", y);
        el.set_property("color", color);

        rt.fontsize = fontsize;
        rt.opacity = opacity;
        rt.x = x;
        rt.y = y;
        rt.position = position;
        rt.launch_opts.fontsize = fontsize;
        rt.launch_opts.opacity = opacity;
        rt.launch_opts.x = x;
        rt.launch_opts.y = y;
        rt.launch_opts.position = position;
        tracing::info!(
            channel_id,
            fontsize,
            opacity,
            x,
            y,
            "TC overlay hot-updated (no relaunch)"
        );
        Ok(())
    }

    fn list_tc_loops(&self) -> Vec<TcLoopSnapshot> {
        let _gst = self.gst_op.lock();
        let mut map = self.tc_loops.lock();
        let ids: Vec<u32> = map.keys().copied().collect();
        for id in ids {
            let Some(rt) = map.get_mut(&id) else {
                continue;
            };
            poll_tc_runtime(rt);
            if let Some(mode) = tc_adapt_candidate(rt) {
                if let Err(e) = relaunch_tc_for_mode(rt, id, &mode) {
                    tracing::warn!(channel_id = id, error = %e, "TC format adapt failed");
                }
            }
        }
        let mut out: Vec<_> = map
            .iter()
            .map(|(id, rt)| tc_runtime_snapshot(*id, rt))
            .collect();
        out.sort_by_key(|t| t.id);
        out
    }

    fn list_tc_loop_meters(&self) -> Vec<TcLoopSnapshot> {
        // Meter tick: peaks only — skip TC format adapt/relaunch.
        let _gst = self.gst_op.lock();
        let mut map = self.tc_loops.lock();
        let ids: Vec<u32> = map.keys().copied().collect();
        for id in ids {
            if let Some(rt) = map.get_mut(&id) {
                poll_tc_runtime(rt);
            }
        }
        let mut out: Vec<_> = map
            .iter()
            .map(|(id, rt)| tc_runtime_snapshot(*id, rt))
            .collect();
        out.sort_by_key(|t| t.id);
        out
    }

    fn tc_loop_snapshot(&self, channel_id: u32) -> Option<TcLoopSnapshot> {
        let _gst = self.gst_op.lock();
        let mut map = self.tc_loops.lock();
        let rt = map.get_mut(&channel_id)?;
        poll_tc_runtime(rt);
        if let Some(mode) = tc_adapt_candidate(rt) {
            if let Err(e) = relaunch_tc_for_mode(rt, channel_id, &mode) {
                tracing::warn!(channel_id, error = %e, "TC format adapt failed");
            }
        }
        Some(tc_runtime_snapshot(channel_id, rt))
    }

    fn set_workflow(&self, channel_id: u32, kind: WorkflowKind, active: bool) -> Result<()> {
        if active {
            if let Some(ch) = self.channels.lock().get(&channel_id) {
                if matches!(ch.status, ChannelStatus::Running | ChannelStatus::Waiting) {
                    bail!("stop encode before enabling exclusive workflow on channel {channel_id}");
                }
            }
        }
        self.workflows.lock().insert(channel_id, (kind, active));
        Ok(())
    }

    fn list_workflows(&self) -> Vec<WorkflowSnapshot> {
        let wf = self.workflows.lock();
        let mut out: Vec<_> = wf
            .iter()
            .map(|(id, (kind, active))| WorkflowSnapshot {
                channel_id: *id,
                kind: kind.clone(),
                active: *active,
                detail: Some(
                    "TC uses cairooverlay path; commentator uses appsrc/appsink (Fas 4)"
                        .into(),
                ),
            })
            .collect();
        out.sort_by_key(|w| w.channel_id);
        out
    }
}
