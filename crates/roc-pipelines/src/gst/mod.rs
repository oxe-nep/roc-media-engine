//! GStreamer-backed pipelines (in-process, no FFmpeg child processes).

mod bitrate;
mod capture;
mod preview_webrtc;
mod probe;

pub use crate::describe::{
    build_capture_launch, build_playout_launch, build_spike_tee_launch, CaptureLaunchOpts,
    PlayoutLaunchOpts,
};
pub use preview_webrtc::WebRtcPreview;
pub use probe::probe_gst_devices;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{anyhow, bail, Context, Result};
use parking_lot::Mutex;
use roc_config::{ChannelConfig, EncodePreset, PlayoutClientConfig};
use roc_devices::DeviceProbeReport;

use crate::{
    ChannelSnapshot, ChannelStatus, PipelineBackend, PlayoutFileControl, PlayoutSnapshot,
    WorkflowKind, WorkflowSnapshot,
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

fn apply_playout_level(peaks: &mut [f64; 2], s: &gstreamer::StructureRef) {
    let Ok(arr) = s.get::<gstreamer::glib::ValueArray>("peak") else {
        return;
    };
    for (i, val) in arr.iter().enumerate().take(2) {
        let Ok(db) = val.get::<f64>() else {
            continue;
        };
        peaks[i] = if db.is_finite() { db.max(-90.0) } else { -90.0 };
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
        let _gst = self.gst_op.lock();
        let mut map = self.channels.lock();
        let pipe = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        let was_live = matches!(pipe.status, ChannelStatus::Running | ChannelStatus::Waiting);
        pipe.stop()?;
        if was_live {
            let prev = self.nvenc_used.fetch_sub(1, Ordering::SeqCst);
            if prev == 0 {
                self.nvenc_used.store(0, Ordering::SeqCst);
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
        let _gst = self.gst_op.lock();
        let mut map = self.channels.lock();
        let pipe = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        pipe.stop_proxy_recording()
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
        let _gst = self.gst_op.lock();
        let mut map = self.channels.lock();
        let pipe = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        pipe.stop_hq_recording()
    }

    fn start_srt(&self, channel_id: u32, url: &str) -> Result<()> {
        let _gst = self.gst_op.lock();
        let mut map = self.channels.lock();
        let pipe = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        pipe.start_srt(url)
    }

    fn stop_srt(&self, channel_id: u32) -> Result<()> {
        let _gst = self.gst_op.lock();
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
        let mut map = self.channels.lock();
        let pipe = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        pipe.start_webrtc_preview(pair, signal_tx)
    }

    fn set_webrtc_answer(&self, channel_id: u32, sdp: &str) -> Result<()> {
        let _gst = self.gst_op.lock();
        let map = self.channels.lock();
        let pipe = map
            .get(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        pipe.set_webrtc_answer(sdp)
    }

    fn add_webrtc_ice(&self, channel_id: u32, sdp_mline_index: u32, candidate: &str) -> Result<()> {
        let _gst = self.gst_op.lock();
        let map = self.channels.lock();
        let pipe = map
            .get(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        pipe.add_webrtc_ice(sdp_mline_index, candidate)
    }

    fn stop_webrtc_preview(&self, channel_id: u32) -> Result<()> {
        let _gst = self.gst_op.lock();
        let mut map = self.channels.lock();
        if let Some(pipe) = map.get_mut(&channel_id) {
            pipe.stop_webrtc_preview();
        }
        Ok(())
    }

    fn stop_webrtc_preview_session(&self, channel_id: u32, session_id: &str) -> Result<()> {
        let _gst = self.gst_op.lock();
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
