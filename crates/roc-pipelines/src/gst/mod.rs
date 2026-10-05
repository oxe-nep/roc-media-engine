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

fn playout_seek_pipeline(pipeline: &gstreamer::Pipeline, position_sec: f64) -> Result<()> {
    use gstreamer::prelude::*;
    let ns = if position_sec.is_finite() && position_sec > 0.0 {
        (position_sec * 1_000_000_000.0).round() as u64
    } else {
        0
    };
    pipeline
        .seek_simple(
            gstreamer::SeekFlags::FLUSH | gstreamer::SeekFlags::KEY_UNIT,
            gstreamer::ClockTime::from_nseconds(ns),
        )
        .map_err(|e| anyhow!("seek to {position_sec:.3}s failed: {e}"))?;
    Ok(())
}

fn query_playout_clock(pipeline: &gstreamer::Pipeline) -> (Option<f64>, Option<f64>) {
    use gstreamer::prelude::*;
    let position_sec = pipeline
        .query_position::<gstreamer::ClockTime>()
        .map(|t| t.nseconds() as f64 / 1_000_000_000.0);
    let duration_sec = pipeline
        .query_duration::<gstreamer::ClockTime>()
        .map(|t| t.nseconds() as f64 / 1_000_000_000.0)
        .filter(|d| d.is_finite() && *d > 0.0);
    (position_sec, duration_sec)
}

fn file_transport_position(p: &PlayoutRuntime) -> f64 {
    let mut pos = p.play_origin_sec.max(0.0);
    if matches!(p.status, ChannelStatus::Running | ChannelStatus::Waiting) {
        if let Some(origin) = p.rate_origin {
            pos += origin.elapsed().as_secs_f64();
        }
    }
    if let Some(d) = p.duration_sec {
        pos = pos.min(d.max(0.0));
    }
    pos
}

fn arm_file_transport(p: &mut PlayoutRuntime, position_sec: f64, running: bool) {
    let mut pos = position_sec.max(0.0);
    if let Some(d) = p.duration_sec {
        pos = pos.min(d.max(0.0));
    }
    p.play_origin_sec = pos;
    p.position_sec = Some(pos);
    p.rate_origin = if running {
        Some(std::time::Instant::now())
    } else {
        None
    };
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
                p.rate_origin = None;
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
                            if p.is_file && p.rate_origin.is_none() {
                                p.rate_origin = Some(std::time::Instant::now());
                            }
                        }
                    }
                }
            }
            MessageView::StateChanged(sc) => {
                if sc.current() == gstreamer::State::Playing
                    && matches!(p.status, ChannelStatus::Waiting)
                {
                    p.status = ChannelStatus::Running;
                    if p.is_file && p.rate_origin.is_none() {
                        p.rate_origin = Some(std::time::Instant::now());
                    }
                }
            }
            _ => {}
        }
    }

    // Refresh duration from GST when available (position from DeckLink pipelines is unreliable).
    if matches!(
        p.status,
        ChannelStatus::Running | ChannelStatus::Paused | ChannelStatus::Waiting
    ) {
        let (_gst_pos, dur) = query_playout_clock(&pipeline);
        if let Some(d) = dur {
            p.duration_sec = Some(d);
        }
    }

    if p.is_file
        && matches!(
            p.status,
            ChannelStatus::Running | ChannelStatus::Paused | ChannelStatus::Waiting
        )
    {
        let pos = file_transport_position(p);
        p.position_sec = Some(pos);
        if matches!(p.status, ChannelStatus::Running) {
            if let Some(out) = p.mark_out_sec {
                if out > p.mark_in_sec && pos >= out - 0.04 {
                    if p.loop_file {
                        seek_to_in = true;
                    } else if let Err(e) = pipeline.set_state(gstreamer::State::Paused) {
                        tracing::warn!(client = %p.name, error = %e, "pause at mark-out failed");
                    } else {
                        p.status = ChannelStatus::Paused;
                        arm_file_transport(p, out, false);
                    }
                }
            } else if let Some(d) = p.duration_sec {
                // Soft end: DeckLink sinks often hold the last frame and never emit EOS.
                if d > 0.0 && pos >= d - 0.04 {
                    if p.loop_file {
                        seek_to_in = true;
                    } else if let Err(e) = pipeline.set_state(gstreamer::State::Paused) {
                        tracing::warn!(client = %p.name, error = %e, "pause at EOF failed");
                    } else {
                        p.status = ChannelStatus::Paused;
                        arm_file_transport(p, d, false);
                    }
                }
            }
        }
    } else if matches!(
        p.status,
        ChannelStatus::Running | ChannelStatus::Paused | ChannelStatus::Waiting
    ) {
        let (pos, _) = query_playout_clock(&pipeline);
        if let Some(pos) = pos {
            p.position_sec = Some(pos);
        }
    }

    if hit_eos {
        if p.is_file && p.loop_file {
            seek_to_in = true;
        } else {
            p.status = ChannelStatus::Stopped;
            p.rate_origin = None;
            p.position_sec = p.duration_sec.or(p.position_sec);
        }
    }

    if seek_to_in {
        let target = p.mark_in_sec.max(0.0);
        match playout_seek_pipeline(&pipeline, target) {
            Ok(()) => {
                let running = !matches!(p.status, ChannelStatus::Paused);
                if matches!(p.status, ChannelStatus::Stopped | ChannelStatus::Paused) {
                    if pipeline.set_state(gstreamer::State::Playing).is_ok() {
                        p.status = ChannelStatus::Running;
                        arm_file_transport(p, target, true);
                    } else {
                        arm_file_transport(p, target, false);
                    }
                } else {
                    arm_file_transport(p, target, running);
                }
            }
            Err(e) => {
                tracing::warn!(client = %p.name, error = %e, "loop seek failed");
                p.status = ChannelStatus::Stopped;
                p.rate_origin = None;
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
    /// Media time at `rate_origin` (file transport clock).
    play_origin_sec: f64,
    /// Wall clock when `play_origin_sec` was anchored (Running only).
    rate_origin: Option<std::time::Instant>,
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
        let launch = build_playout_launch(&PlayoutLaunchOpts {
            source: source.to_string(),
            device: client.device.clone(),
            format_code: format_code.clone(),
            hls_dir,
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
        let (_, duration_sec) = query_playout_clock(&pipeline);
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
                play_origin_sec: 0.0,
                rate_origin: if is_file && matches!(initial, ChannelStatus::Running) {
                    Some(std::time::Instant::now())
                } else {
                    None
                },
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
            p.rate_origin = None;
            p.play_origin_sec = 0.0;
        }
        Ok(())
    }

    fn pause_playout(&self, client_id: &str) -> Result<()> {
        use gstreamer::prelude::*;
        let mut map = self.playout.lock();
        let p = map
            .get_mut(client_id)
            .with_context(|| format!("playout {client_id} not running"))?;
        if p.is_file {
            let pos = file_transport_position(p);
            arm_file_transport(p, pos, false);
        }
        let pipe = p
            .pipeline
            .as_ref()
            .with_context(|| format!("playout {client_id} has no pipeline"))?
            .clone();
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
            let pos = file_transport_position(p);
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
            if (target - pos).abs() > 0.02 {
                playout_seek_pipeline(&pipe, target)?;
            }
            arm_file_transport(p, target, true);
        }
        pipe.set_state(gstreamer::State::Playing)
            .context("playout PLAYING")?;
        p.status = ChannelStatus::Running;
        Ok(())
    }

    fn seek_playout(&self, client_id: &str, position_sec: f64) -> Result<()> {
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
        playout_seek_pipeline(&pipe, target)?;
        let running = matches!(p.status, ChannelStatus::Running | ChannelStatus::Waiting);
        arm_file_transport(p, target, running);
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
                    let _ = playout_seek_pipeline(pipe, out);
                    let _ = pipe.set_state(gstreamer::State::Paused);
                    p.status = ChannelStatus::Paused;
                    arm_file_transport(p, out, false);
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
