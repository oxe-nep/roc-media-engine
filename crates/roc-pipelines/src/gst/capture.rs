//! Per-channel GStreamer capture graph with dynamic REC/SRT branches (no full relaunch).

use anyhow::{anyhow, bail, Context, Result};
use gstreamer::prelude::*;
use roc_config::{ChannelConfig, EncodePreset};
use std::str::FromStr;

use super::bitrate::BitrateMeter;
use crate::describe::{build_capture_encode_once_launch, CaptureLaunchOpts};
use crate::signal_format::{format_from_caps, is_auto_mode, probe_input_format, InputFormat};
use crate::{ChannelSnapshot, ChannelStatus};

/// Insert an `identity` that forces a clean TIME segment from 0.
///
/// Late-join REC/SRT sees video PTS in a different domain than live AAC
/// (observed: ~1e15 ns vs ~1e9 ns). Without a segment reset, mp4mux/mpegtsmux
/// remaps each pad independently and lipsync drifts. `single-segment=true`
/// rewrites timestamps onto a fresh segment; pair with audio
/// `min-threshold-time` so AAC is held roughly one NVENC latency behind.
fn make_mux_ts_align(name: &str) -> Result<gstreamer::Element> {
    gstreamer::ElementFactory::make("identity")
        .name(name)
        .property("single-segment", true)
        .property("silent", true)
        .build()
        .context("identity single-segment")
}

fn install_mux_av_sync_log(tag: &str) {
    // Sync is applied via identity + keyframe-gated AAC; this log keeps
    // deploy/verify scripts discoverable.
    tracing::info!(%tag, "mux A/V sync: identity single-segment + keyframe-gated AAC");
}

/// Drop AAC buffers until `gate` is opened (first video keyframe).
/// Installed on the audio queue sink so stale pre-keyframe PCM never buffers.
fn install_av_start_gate(audio_queue: &gstreamer::Element, gate: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    use gstreamer::{PadProbeReturn, PadProbeType};
    use std::sync::atomic::Ordering;

    let Some(pad) = audio_queue.static_pad("sink") else {
        return;
    };
    pad.add_probe(PadProbeType::BUFFER, move |_, _info| {
        if gate.load(Ordering::SeqCst) {
            PadProbeReturn::Ok
        } else {
            PadProbeReturn::Drop
        }
    });
}

/// Open `gate` when the first non-delta (key) video buffer is seen.
/// Also drops leading delta units so identity single-segment starts on the IDR.
fn arm_av_gate_on_keyframe(video_identity: &gstreamer::Element, gate: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    use gstreamer::{PadProbeReturn, PadProbeType};
    use std::sync::atomic::Ordering;

    // Probe the *sink* so single-segment sees the keyframe as its first buffer.
    let Some(pad) = video_identity.static_pad("sink") else {
        return;
    };
    pad.add_probe(PadProbeType::BUFFER, move |_, info| {
        let Some(buf) = info.buffer() else {
            return PadProbeReturn::Ok;
        };
        if buf.flags().contains(gstreamer::BufferFlags::DELTA_UNIT) {
            return PadProbeReturn::Drop;
        }
        gate.store(true, Ordering::SeqCst);
        tracing::info!("mux A/V sync: opened AAC gate on video keyframe");
        PadProbeReturn::Remove
    });
}

/// Hold MPEG-TS at a `valve` until video (and audio, if present) have each
/// delivered one buffer into `mpegtsmux`. MediaMTX locks tracks from the first
/// PMT it sees — emitting PES before both pads are live → "undeclared track".
fn arm_srt_pmt_ready_valve(
    valve: &gstreamer::Element,
    video_src: &gstreamer::Pad,
    audio_src: Option<&gstreamer::Pad>,
) {
    use gstreamer::{PadProbeReturn, PadProbeType};
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
    use std::sync::Arc;

    let _ = valve.set_property("drop", true);
    let video_ok = Arc::new(AtomicBool::new(false));
    let audio_ok = Arc::new(AtomicBool::new(audio_src.is_none()));
    let opened = Arc::new(AtomicU8::new(0));
    let valve = valve.clone();

    let try_open = {
        let video_ok = video_ok.clone();
        let audio_ok = audio_ok.clone();
        let opened = opened.clone();
        let valve = valve.clone();
        move || {
            if video_ok.load(Ordering::SeqCst) && audio_ok.load(Ordering::SeqCst) {
                if opened
                    .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    let _ = valve.set_property("drop", false);
                    tracing::info!("SRT PMT gate open — A/V both seen before first TS out");
                }
            }
        }
    };

    {
        let video_ok = video_ok.clone();
        let try_open = try_open.clone();
        video_src.add_probe(PadProbeType::BUFFER, move |_, _| {
            video_ok.store(true, Ordering::SeqCst);
            try_open();
            PadProbeReturn::Ok
        });
    }
    if let Some(ap) = audio_src {
        let audio_ok = audio_ok.clone();
        let try_open = try_open.clone();
        ap.add_probe(PadProbeType::BUFFER, move |_, _| {
            audio_ok.store(true, Ordering::SeqCst);
            try_open();
            PadProbeReturn::Ok
        });
    }

    // Never leave SRT silent forever if one pad never produces (e.g. silent audio).
    std::thread::spawn({
        let opened = opened.clone();
        let valve = valve.clone();
        move || {
            std::thread::sleep(std::time::Duration::from_secs(2));
            if opened
                .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                let _ = valve.set_property("drop", false);
                tracing::warn!("SRT PMT gate timeout — opening after 2s");
            }
        }
    });
}

/// Go/FFmpeg `OutputURL` puts `latency` in **microseconds**; GStreamer/libsrt URI
/// expects **milliseconds**. Values > 8000 are treated as µs (same heuristic as Go).
fn normalize_srt_uri_for_gst(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(i) = rest.find("latency=") {
        out.push_str(&rest[..i]);
        out.push_str("latency=");
        rest = &rest[i + "latency=".len()..];
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let num = &rest[..end];
        if let Ok(v) = num.parse::<u64>() {
            let ms = if v > 8000 { (v / 1000).max(1) } else { v };
            out.push_str(&ms.to_string());
        } else {
            out.push_str(num);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod srt_uri_tests {
    use super::normalize_srt_uri_for_gst;

    #[test]
    fn converts_ffmpeg_micros_to_gst_millis() {
        let u = normalize_srt_uri_for_gst(
            "srt://10.0.0.1:8890?latency=1000000&mode=caller&pkt_size=1316",
        );
        assert!(u.contains("latency=1000"), "{u}");
        assert!(u.contains("mode=caller"), "{u}");
    }

    #[test]
    fn leaves_millis_alone() {
        let u = normalize_srt_uri_for_gst("srt://0.0.0.0:9101?mode=listener&latency=120");
        assert!(u.contains("latency=120"), "{u}");
    }
}

pub struct ChannelPipeline {
    pub id: u32,
    pub name: String,
    /// Config key (`hq`, `proxy`, …) — what UI/Go send on preset change.
    pub encode_preset_id: String,
    pub encode_preset_label: String,
    pub status: ChannelStatus,
    pub recording: bool,
    pub srt: bool,
    pub recording_path: Option<String>,
    pub srt_url: Option<String>,
    pub last_error: Option<String>,
    device: String,
    /// User config: `auto` or a concrete DeckLink mode.
    configured_mode: String,
    /// Mode locked into the running graph.
    locked_mode: String,
    detected: Option<InputFormat>,
    preset: EncodePreset,
    udp_egress: Option<String>,
    parse_element: String,
    pipeline: Option<gstreamer::Pipeline>,
    rec_branch: Option<Branch>,
    srt_branch: Option<Branch>,
    /// Avoid relaunch storms: last adapt attempt.
    last_adapt: Option<std::time::Instant>,
    /// Peak dBFS per discrete channel (8). Updated from `level` bus messages.
    audio_peaks: [f64; 8],
    /// Live encoded bitstream rate (tee `e` sink probe).
    encode_bitrate: BitrateMeter,
    /// Live SRT MPEG-TS rate (mpegtsmux → srtsink), only while SRT attached.
    srt_bitrate: Option<BitrateMeter>,
}

struct Branch {
    tee_pad: gstreamer::Pad,
    /// Optional pad from audio tee `a` (SRT/REC with AAC).
    audio_tee_pad: Option<gstreamer::Pad>,
    elements: Vec<gstreamer::Element>,
}

fn parse_element_for_codec(video_codec: &str) -> &'static str {
    let c = video_codec.to_ascii_lowercase();
    if c.contains("265") || c.contains("hevc") {
        "h265parse"
    } else {
        "h264parse"
    }
}

impl ChannelPipeline {
    pub fn new(ch: &ChannelConfig, preset: &EncodePreset) -> Result<Self> {
        let preset_id = ch
            .encode_preset
            .clone()
            .unwrap_or_else(|| "hq".into());
        Ok(Self {
            id: ch.id,
            name: ch.name.clone(),
            encode_preset_id: preset_id,
            encode_preset_label: preset.label.clone(),
            status: ChannelStatus::Stopped,
            recording: false,
            srt: false,
            recording_path: None,
            srt_url: ch.srt_url.clone(),
            last_error: None,
            device: ch.device.clone(),
            configured_mode: ch
                .mode
                .clone()
                .unwrap_or_else(|| "auto".into()),
            locked_mode: String::new(),
            detected: None,
            parse_element: parse_element_for_codec(&preset.video_codec).to_string(),
            preset: preset.clone(),
            udp_egress: ch.udp_egress.clone(),
            pipeline: None,
            rec_branch: None,
            srt_branch: None,
            last_adapt: None,
            audio_peaks: [-90.0; 8],
            encode_bitrate: BitrateMeter::new(),
            srt_bitrate: None,
        })
    }

    pub fn update_config(&mut self, ch: &ChannelConfig, preset: &EncodePreset) {
        self.name = ch.name.clone();
        self.device = ch.device.clone();
        self.configured_mode = ch.mode.clone().unwrap_or_else(|| "auto".into());
        self.preset = preset.clone();
        if let Some(id) = &ch.encode_preset {
            self.encode_preset_id = id.clone();
        }
        self.encode_preset_label = preset.label.clone();
        self.parse_element = parse_element_for_codec(&preset.video_codec).to_string();
        self.udp_egress = ch.udp_egress.clone();
        if self.srt_url.is_none() {
            self.srt_url = ch.srt_url.clone();
        }
    }

    /// Apply a new encode preset. Relunches the capture graph when live (preserves REC/SRT).
    pub fn apply_encode_preset(&mut self, preset_id: &str, preset: &EncodePreset) -> Result<()> {
        if self.recording {
            bail!("stop recording before changing encode preset");
        }
        let live = matches!(
            self.status,
            ChannelStatus::Running | ChannelStatus::Waiting
        );
        self.encode_preset_id = preset_id.to_string();
        self.encode_preset_label = preset.label.clone();
        self.preset = preset.clone();
        self.parse_element = parse_element_for_codec(&preset.video_codec).to_string();
        if !live {
            tracing::info!(
                channel = self.id,
                preset_id,
                "encode preset updated (applies on next start)"
            );
            return Ok(());
        }
        let mode = if self.locked_mode.is_empty() {
            self.configured_mode.clone()
        } else {
            self.locked_mode.clone()
        };
        tracing::info!(
            channel = self.id,
            preset_id,
            %mode,
            "encode preset changed — relaunching capture"
        );
        self.relaunch_preserving_branches(&mode)
    }

    fn relaunch_preserving_branches(&mut self, mode: &str) -> Result<()> {
        let was_rec = self.recording;
        let rec_path = self.recording_path.clone();
        let was_srt = self.srt;
        let srt_url = self.srt_url.clone();

        let _ = self.detach_recording(false);
        let _ = self.detach_srt(false);
        if let Some(p) = self.pipeline.take() {
            let _ = p.set_state(gstreamer::State::Null);
        }
        self.recording = false;
        self.srt = false;
        self.recording_path = None;
        if mode != "auto" && !mode.is_empty() {
            self.locked_mode = mode.to_string();
        }
        let launch_mode = if self.locked_mode.is_empty() {
            "auto".to_string()
        } else {
            self.locked_mode.clone()
        };
        self.launch_locked(&launch_mode)?;

        if was_rec {
            if let Some(path) = rec_path {
                let _ = self.start_recording(&path);
            }
        }
        if was_srt {
            if let Some(url) = srt_url {
                let _ = self.start_srt(&url);
            }
        }
        Ok(())
    }

    pub fn start(&mut self) -> Result<()> {
        if self.pipeline.is_some() {
            return Ok(());
        }
        let locked = self.resolve_lock_mode()?;
        self.locked_mode = locked.clone();
        self.launch_locked(&locked)?;
        Ok(())
    }

    fn resolve_lock_mode(&mut self) -> Result<String> {
        if !is_auto_mode(&self.configured_mode) {
            return Ok(self.configured_mode.clone());
        }
        match probe_input_format(&self.device, 3500) {
            Ok(fmt) => {
                tracing::info!(
                    channel = self.id,
                    format = %fmt.summary(),
                    "probed input format"
                );
                self.detected = Some(fmt.clone());
                Ok(fmt.mode)
            }
            Err(err) => {
                tracing::warn!(
                    channel = self.id,
                    error = %err,
                    "input probe failed — falling back to 1080p50"
                );
                Ok("1080p50".into())
            }
        }
    }

    fn launch_locked(&mut self, locked: &str) -> Result<()> {
        let hls_dir = format!("/opt/application/roc-recording/backend/hls/{}", self.id);
        let _ = std::fs::create_dir_all(&hls_dir);
        // Drop stale preview playlists/segments so players don't stick on old gens.
        if let Ok(rd) = std::fs::read_dir(&hls_dir) {
            for ent in rd.flatten() {
                let name = ent.file_name();
                let n = name.to_string_lossy();
                if n == "preview.m3u8"
                    || n.ends_with(".ts")
                    || n.starts_with("listen_")
                    || n.starts_with("pv")
                    || n == "thumb.jpg"
                {
                    let _ = std::fs::remove_file(ent.path());
                }
            }
        }
        let playlist = format!("{hls_dir}/preview.m3u8");
        let launch = build_capture_encode_once_launch(&CaptureLaunchOpts {
            device: self.device.clone(),
            mode: locked.to_string(),
            preset: self.preset.clone(),
            preview_path: Some(playlist),
            record_path: None,
            srt_url: None,
            udp_egress: self.udp_egress.clone(),
            with_tee_preview: true,
        });
        tracing::info!(channel = self.id, mode = %locked, %launch, "capture pipeline");
        let pipeline = gstreamer::parse::launch(&launch)
            .with_context(|| format!("parse capture launch ch{}", self.id))?
            .downcast::<gstreamer::Pipeline>()
            .map_err(|_| anyhow!("capture launch did not yield Pipeline"))?;

        pipeline
            .set_state(gstreamer::State::Playing)
            .context("capture set PLAYING")?;
        // Live encode bitrate: count buffers into encoded tee `e`.
        self.encode_bitrate.reset();
        if let Some(tee) = pipeline.by_name("e") {
            if let Some(sink) = tee.static_pad("sink") {
                let _ = self.encode_bitrate.attach_probe(&sink);
            }
        }
        self.srt_bitrate = None;
        self.pipeline = Some(pipeline);
        self.status = ChannelStatus::Waiting;
        self.last_error = None;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        let _ = self.detach_recording(false);
        let _ = self.detach_srt(false);
        if let Some(p) = self.pipeline.take() {
            let _ = p.send_event(gstreamer::event::Eos::new());
            let _ = p.set_state(gstreamer::State::Null);
        }
        self.status = ChannelStatus::Stopped;
        self.recording = false;
        self.srt = false;
        self.recording_path = None;
        self.audio_peaks = [-90.0; 8];
        self.encode_bitrate.reset();
        self.srt_bitrate = None;
        Ok(())
    }

    pub fn start_recording(&mut self, path: &str) -> Result<()> {
        if self.pipeline.is_none() {
            bail!("capture not running");
        }
        if self.recording {
            bail!("already recording");
        }
        self.attach_recording(path)?;
        self.recording = true;
        self.recording_path = Some(path.to_string());
        Ok(())
    }

    pub fn stop_recording(&mut self) -> Result<()> {
        if !self.recording {
            return Ok(());
        }
        self.detach_recording(true)?;
        self.recording = false;
        self.recording_path = None;
        Ok(())
    }

    pub fn start_srt(&mut self, url: &str) -> Result<()> {
        if self.pipeline.is_none() {
            bail!("capture not running");
        }
        if self.srt {
            let _ = self.detach_srt(false);
        }
        self.srt_url = Some(url.to_string());
        self.attach_srt(url)?;
        self.srt = true;
        Ok(())
    }

    pub fn stop_srt(&mut self) -> Result<()> {
        if !self.srt {
            return Ok(());
        }
        self.detach_srt(false)?;
        self.srt = false;
        Ok(())
    }

    fn encoded_tee(&self) -> Result<gstreamer::Element> {
        let p = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow!("no pipeline"))?;
        p.by_name("e")
            .ok_or_else(|| anyhow!("encoded tee `e` missing — is capture running?"))
    }

    fn attach_recording(&mut self, path: &str) -> Result<()> {
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow!("no pipeline"))?
            .clone();
        let tee = self.encoded_tee()?;
        let audio_tee = pipeline.by_name("a");

        let queue_v = gstreamer::ElementFactory::make("queue")
            .name(format!("q_rec_v_{}", self.id))
            .build()
            .context("queue video")?;
        let parse = gstreamer::ElementFactory::make(&self.parse_element)
            .name(format!("parse_rec_{}", self.id))
            .build()
            .with_context(|| format!("make {}", self.parse_element))?;
        // Keep SPS/PPS (or VPS/SPS/PPS for HEVC) in-band for mid-stream joiners.
        let _ = parse.set_property_from_str("config-interval", "-1");
        // Progressive MP4 (moov at end). Fragmented/streamable files often look
        // "corrupt" in browsers and desktop players when finalize is incomplete.
        let mux = gstreamer::ElementFactory::make("mp4mux")
            .name(format!("mux_rec_{}", self.id))
            .property("fragment-duration", 0u32)
            .property("streamable", false)
            .build()
            .context("mp4mux")?;
        let sink = gstreamer::ElementFactory::make("filesink")
            .name(format!("fs_rec_{}", self.id))
            .property("location", path)
            .property("sync", false)
            // Ensure CIFS/NFS sees bytes promptly; helps avoid 0-byte stubs on stop.
            .property("async", false)
            .build()
            .context("filesink")?;

        let id_v = make_mux_ts_align(&format!("id_rec_v_{}", self.id))?;
        let mut elements = vec![
            queue_v.clone(),
            parse.clone(),
            id_v.clone(),
            mux.clone(),
            sink.clone(),
        ];
        pipeline.add_many([&queue_v, &parse, &id_v, &mux, &sink])?;
        queue_v.link(&parse).context("link rec queue→parse")?;
        parse.link(&id_v).context("link rec parse→identity")?;
        id_v.link(&mux).context("link rec identity→mux")?;
        mux.link(&sink).context("link rec mux→sink")?;

        // Attach AAC now so mp4mux registers the audio track, but gate buffers
        // until the first video keyframe. Otherwise AAC starts immediately while
        // video waits up to one GOP (~1s) for IDR — both get pts=0 via
        // single-segment → permanent ~1s lipsync error.
        let av_gate = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut audio_tee_pad = None;
        if let Some(a_tee) = audio_tee {
            match self.link_program_aac(&pipeline, &a_tee, &mux, "rec", true) {
                Ok((a_pad, audio_els)) => {
                    // audio_els: [queue_a, abin, id_a] — gate on queue sink.
                    if let Some(q_a) = audio_els.first() {
                        install_av_start_gate(q_a, av_gate.clone());
                    }
                    audio_tee_pad = Some(a_pad);
                    elements.extend(audio_els);
                    install_mux_av_sync_log("rec");
                }
                Err(err) => {
                    tracing::warn!(
                        channel = self.id,
                        error = %err,
                        "REC video-only — program AAC attach failed"
                    );
                }
            }
        } else {
            tracing::warn!(
                channel = self.id,
                "REC video-only — audio tee `a` missing"
            );
        }

        for el in &elements {
            el.sync_state_with_parent()
                .context("sync_state_with_parent record")?;
        }

        // Open the audio gate on the first video keyframe (non-DELTA_UNIT).
        if audio_tee_pad.is_some() {
            arm_av_gate_on_keyframe(&id_v, av_gate);
        }

        let tee_pad = tee
            .request_pad_simple("src_%u")
            .ok_or_else(|| anyhow!("tee request_pad failed"))?;
        let sink_pad = queue_v
            .static_pad("sink")
            .ok_or_else(|| anyhow!("queue sink pad"))?;
        tee_pad
            .link(&sink_pad)
            .context("link tee → record queue")?;

        tracing::info!(
            channel = self.id,
            %path,
            with_audio = audio_tee_pad.is_some(),
            "attached record branch (no relaunch)"
        );
        self.rec_branch = Some(Branch {
            tee_pad,
            audio_tee_pad,
            elements,
        });
        Ok(())
    }

    /// Stereo pair 1–2 → AAC into an existing mux (mp4mux / mpegtsmux).
    ///
    /// `ts_align`: insert `identity single-segment` (needed for REC mp4mux late-join).
    /// Leave false for live MPEG-TS/SRT — gating/restamp there makes PMT video-only.
    fn link_program_aac(
        &self,
        pipeline: &gstreamer::Pipeline,
        a_tee: &gstreamer::Element,
        mux: &gstreamer::Element,
        tag: &str,
        ts_align: bool,
    ) -> Result<(gstreamer::Pad, Vec<gstreamer::Element>)> {
        let hold_ms = if ts_align { 40u64 } else { 0 };
        let queue_a = gstreamer::ElementFactory::make("queue")
            .name(format!("q_{tag}_a_{}", self.id))
            .property("min-threshold-time", gstreamer::ClockTime::from_mseconds(hold_ms))
            .property("max-size-buffers", 64u32)
            .property("max-size-bytes", 0u32)
            .property("max-size-time", gstreamer::ClockTime::from_mseconds(250))
            .build()
            .context("queue audio")?;
        let aac_bps = crate::parse_bitrate(&self.preset.audio_bitrate).unwrap_or(192_000);
        // MediaMTX (and MPEG-TS stream type 0x0F) expects ADTS-framed AAC, not raw.
        // aacparse defaults can stay raw → undeclared PID / no Opus remux downstream.
        let desc = format!(
            "audioconvert mix-matrix=\"<<1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0>, \
             <0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0>>\" ! \
             audio/x-raw,channels=2 ! voaacenc bitrate={aac_bps} ! aacparse ! \
             capsfilter caps=audio/mpeg,mpegversion=4,stream-format=adts"
        );
        let abin = gstreamer::parse::bin_from_description(&desc, true)
            .with_context(|| format!("parse {tag} audio bin"))?;
        abin.set_property("name", format!("{tag}_a_bin_{}", self.id));
        let abin_el: gstreamer::Element = abin.upcast();

        let mut els = vec![queue_a.clone(), abin_el.clone()];
        pipeline
            .add_many([&queue_a, &abin_el])
            .context("add program audio elements")?;
        queue_a
            .link(&abin_el)
            .context("link audio queue → bin")?;

        if ts_align {
            let id_a = make_mux_ts_align(&format!("id_{tag}_a_{}", self.id))?;
            pipeline.add(&id_a).context("add audio identity")?;
            abin_el
                .link(&id_a)
                .context("link audio bin → identity")?;
            id_a
                .link(mux)
                .context("link audio identity → mux")?;
            els.push(id_a);
        } else {
            abin_el
                .link(mux)
                .context("link audio bin → mux")?;
        }

        let a_pad = a_tee
            .request_pad_simple("src_%u")
            .ok_or_else(|| anyhow!("audio tee request_pad failed"))?;
        let a_sink = queue_a
            .static_pad("sink")
            .ok_or_else(|| anyhow!("audio queue sink"))?;
        a_pad
            .link(&a_sink)
            .context("link audio tee → queue")?;
        Ok((a_pad, els))
    }

    fn detach_recording(&mut self, finalize: bool) -> Result<()> {
        let Some(branch) = self.rec_branch.take() else {
            return Ok(());
        };
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow!("no pipeline"))?
            .clone();
        let tee = self.encoded_tee()?;

        // 1) Cut data path from tee before touching downstream state.
        Self::unlink_branch(&pipeline, &tee, &branch);

        // 2) Optional EOS for a cleaner mp4 footer. Do NOT wait on the pipeline bus:
        //    that raced under multi-channel stop and held the global lock for seconds.
        if finalize {
            use gstreamer::{PadProbeReturn, PadProbeType};
            use std::sync::mpsc;

            let (tx, rx) = mpsc::channel::<()>();
            // Keep probe id alive until after recv — dropping it removes the probe.
            let eos_probe = branch
                .elements
                .iter()
                .find(|e| e.name().starts_with("fs_rec"))
                .and_then(|sink| sink.static_pad("sink"))
                .map(|pad| {
                    let tx = tx.clone();
                    pad.add_probe(PadProbeType::EVENT_DOWNSTREAM, move |_, info| {
                        if let Some(ev) = info.event() {
                            if ev.type_() == gstreamer::EventType::Eos {
                                let _ = tx.send(());
                                return PadProbeReturn::Remove;
                            }
                        }
                        PadProbeReturn::Ok
                    })
                });
            drop(tx);

            // Inject EOS *into* each record queue sink (downstream). Element-level
            // send_event(EOS) on a filter goes to its sink pads (upstream) and
            // never reaches mp4mux — leaving a moov-less "corrupt" file.
            for el in &branch.elements {
                let name = el.name();
                if name.starts_with("q_rec") {
                    if let Some(sink) = el.static_pad("sink") {
                        let _ = sink.send_event(gstreamer::event::Eos::new());
                    }
                }
            }
            // Progressive mp4mux writes moov only on EOS — wait generously.
            match rx.recv_timeout(std::time::Duration::from_millis(3000)) {
                Ok(()) => tracing::info!(channel = self.id, "record EOS reached filesink"),
                Err(_) => tracing::warn!(
                    channel = self.id,
                    "record EOS timeout — moov may be missing"
                ),
            }
            drop(eos_probe);
        }

        // 3) Null from sink → source, then remove.
        for el in branch.elements.iter().rev() {
            let _ = el.set_state(gstreamer::State::Null);
        }
        for el in &branch.elements {
            let _ = pipeline.remove(el);
        }
        tracing::info!(channel = self.id, "detached record branch");
        Ok(())
    }

    fn unlink_branch(
        pipeline: &gstreamer::Pipeline,
        video_tee: &gstreamer::Element,
        branch: &Branch,
    ) {
        if let Some(qpad) = branch.elements.first().and_then(|e| e.static_pad("sink")) {
            let _ = branch.tee_pad.unlink(&qpad);
        }
        video_tee.release_request_pad(&branch.tee_pad);
        if let Some(ref audio_pad) = branch.audio_tee_pad {
            if let Some(peer) = audio_pad.peer() {
                let _ = audio_pad.unlink(&peer);
            }
            if let Some(a_tee) = pipeline.by_name("a") {
                a_tee.release_request_pad(audio_pad);
            }
        }
    }

    fn attach_srt(&mut self, url: &str) -> Result<()> {
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow!("no pipeline"))?
            .clone();
        let tee = self.encoded_tee()?;
        let audio_tee = pipeline.by_name("a");
        let gst_url = normalize_srt_uri_for_gst(url);
        if gst_url != url {
            tracing::info!(
                channel = self.id,
                from = %url,
                to = %gst_url,
                "normalized SRT URI latency for GStreamer (µs→ms)"
            );
        }

        let queue_v = gstreamer::ElementFactory::make("queue")
            .name(format!("q_srt_v_{}", self.id))
            .build()
            .context("queue video")?;
        let parse = gstreamer::ElementFactory::make(&self.parse_element)
            .name(format!("parse_srt_{}", self.id))
            .build()
            .with_context(|| format!("make {}", self.parse_element))?;
        // Critical for players joining mid-stream (MediaMTX/VLC): repeat parameter sets
        // on every IDR. Without this, many clients get AAC audio only and no video.
        let _ = parse.set_property_from_str("config-interval", "-1");
        // Annex-B into mpegtsmux — avc/hvc1 length-prefixed NALs produce undecodable TS.
        let bs_caps = if self.parse_element.contains("265") {
            "video/x-h265,stream-format=byte-stream,alignment=au"
        } else {
            "video/x-h264,stream-format=byte-stream,alignment=au"
        };
        let capsfilter = gstreamer::ElementFactory::make("capsfilter")
            .name(format!("cf_srt_{}", self.id))
            .property(
                "caps",
                gstreamer::Caps::from_str(bs_caps).context("srt byte-stream caps")?,
            )
            .build()
            .context("srt capsfilter")?;
        let mux = gstreamer::ElementFactory::make("mpegtsmux")
            .name(format!("mux_srt_{}", self.id))
            .property("alignment", 7i32)
            .build()
            .context("mpegtsmux")?;
        // Drop TS until both A/V have entered the mux (complete first PMT).
        let valve = gstreamer::ElementFactory::make("valve")
            .name(format!("valve_srt_{}", self.id))
            .property("drop", true)
            .build()
            .context("srt pmt valve")?;
        // Bounded leaky queue so a disconnected peer cannot grow RAM forever.
        let q_br = gstreamer::ElementFactory::make("queue")
            .name(format!("q_srt_br_{}", self.id))
            .property_from_str("leaky", "downstream")
            .property("max-size-buffers", 30u32)
            .property("max-size-bytes", 0u32)
            .property("max-size-time", 0u64)
            .build()
            .context("srt bitrate queue")?;
        // wait-for-connection=false: never stall the shared capture graph if the
        // peer is down; auto-reconnect keeps trying (caller → MediaMTX).
        let sink = gstreamer::ElementFactory::make("srtsink")
            .name(format!("srt_sink_{}", self.id))
            .property("uri", &gst_url)
            .property("wait-for-connection", false)
            .property("auto-reconnect", true)
            .build()
            .context("srtsink")?;

        let mut elements = vec![
            queue_v.clone(),
            parse.clone(),
            capsfilter.clone(),
            mux.clone(),
            valve.clone(),
            q_br.clone(),
            sink.clone(),
        ];
        pipeline.add_many([
            &queue_v,
            &parse,
            &capsfilter,
            &mux,
            &valve,
            &q_br,
            &sink,
        ])?;
        queue_v.link(&parse).context("link srt queue→parse")?;
        parse
            .link(&capsfilter)
            .context("link srt parse→capsfilter")?;
        capsfilter.link(&mux).context("link srt capsfilter→mux")?;
        mux.link(&valve).context("link srt mux→valve")?;
        valve.link(&q_br).context("link srt valve→bitrate queue")?;
        q_br.link(&sink).context("link srt bitrate queue→sink")?;

        // Do not keyframe-gate / single-segment here — that delays AAC and yields
        // a video-only first PMT (MediaMTX then ignores audio PID 66).
        let mut audio_tee_pad = None;
        let mut audio_src_pad = None;
        if let Some(a_tee) = audio_tee {
            match self.link_program_aac(&pipeline, &a_tee, &mux, "srt", false) {
                Ok((a_pad, audio_els)) => {
                    // Last element before mux is the AAC bin (or identity if aligned).
                    if let Some(last) = audio_els.last() {
                        audio_src_pad = last.static_pad("src");
                    }
                    audio_tee_pad = Some(a_pad);
                    elements.extend(audio_els);
                }
                Err(err) => {
                    tracing::warn!(
                        channel = self.id,
                        error = %err,
                        "SRT video-only — program AAC attach failed"
                    );
                }
            }
        } else {
            tracing::warn!(
                channel = self.id,
                "SRT video-only — audio tee `a` missing (preview/listen not in graph)"
            );
        }

        let video_src = capsfilter
            .static_pad("src")
            .ok_or_else(|| anyhow!("srt video capsfilter src"))?;
        arm_srt_pmt_ready_valve(&valve, &video_src, audio_src_pad.as_ref());

        for el in &elements {
            el.sync_state_with_parent()
                .context("sync_state_with_parent srt")?;
        }

        let srt_meter = BitrateMeter::new();
        // Count encoded video entering the SRT branch (flows even if srtsink has no peer).
        if let Some(pad) = queue_v.static_pad("src") {
            let _ = srt_meter.attach_probe(&pad);
        }
        self.srt_bitrate = Some(srt_meter);

        let tee_pad = tee
            .request_pad_simple("src_%u")
            .ok_or_else(|| anyhow!("video tee request_pad failed"))?;
        let sink_pad = queue_v
            .static_pad("sink")
            .ok_or_else(|| anyhow!("srt video queue sink"))?;
        tee_pad
            .link(&sink_pad)
            .context("link video tee → srt queue")?;

        // Keep the operator-facing URL (may still be FFmpeg µs form) for API/UI.
        self.srt_url = Some(url.to_string());

        tracing::info!(
            channel = self.id,
            %url,
            gst_uri = %gst_url,
            with_audio = audio_tee_pad.is_some(),
            "attached SRT branch (no relaunch)"
        );
        self.srt_branch = Some(Branch {
            tee_pad,
            audio_tee_pad,
            elements,
        });
        Ok(())
    }

    fn detach_srt(&mut self, _finalize: bool) -> Result<()> {
        let Some(branch) = self.srt_branch.take() else {
            return Ok(());
        };
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow!("no pipeline"))?
            .clone();
        let tee = self.encoded_tee()?;

        Self::unlink_branch(&pipeline, &tee, &branch);

        for el in branch.elements.iter().rev() {
            let _ = el.set_state(gstreamer::State::Null);
        }
        for el in &branch.elements {
            let _ = pipeline.remove(el);
        }
        self.srt_bitrate = None;
        tracing::info!(channel = self.id, "detached SRT branch");
        Ok(())
    }

    pub fn poll_bus(&mut self) {
        {
            let Some(p) = self.pipeline.as_ref() else {
                return;
            };
            let (_, cur, _) = p.state(gstreamer::ClockTime::ZERO);
            if cur == gstreamer::State::Playing && self.status != ChannelStatus::Error {
                self.status = ChannelStatus::Running;
            }
        }

        let adapt_to = {
            if let Some(fmt) = self.read_live_format() {
                let changed = self
                    .detected
                    .as_ref()
                    .map(|d| d.mode != fmt.mode)
                    .unwrap_or(true);
                self.detected = Some(fmt.clone());
                let should = is_auto_mode(&self.configured_mode)
                    && fmt.mode != self.locked_mode
                    && fmt.width >= 1280
                    && self
                        .last_adapt
                        .map(|t| t.elapsed() > std::time::Duration::from_secs(3))
                        .unwrap_or(true);
                if should {
                    Some(fmt.mode)
                } else {
                    if changed {
                        tracing::info!(
                            channel = self.id,
                            format = %fmt.summary(),
                            "input format"
                        );
                    }
                    None
                }
            } else {
                None
            }
        };
        if let Some(mode) = adapt_to {
            tracing::warn!(
                channel = self.id,
                from = %self.locked_mode,
                to = %mode,
                "input format changed — adapting capture"
            );
            let _ = self.adapt_to_mode(&mode);
        }

        let Some(p) = self.pipeline.as_ref() else {
            return;
        };
        let bus = p.bus().expect("pipeline bus");
        while let Some(msg) = bus.timed_pop(gstreamer::ClockTime::ZERO) {
            use gstreamer::MessageView;
            match msg.view() {
                MessageView::Error(err) => {
                    self.status = ChannelStatus::Error;
                    self.last_error = Some(format!(
                        "{} ({})",
                        err.error(),
                        err.debug().unwrap_or_default()
                    ));
                    tracing::error!(channel = self.id, error = ?self.last_error, "gst error");
                }
                MessageView::Eos(_) => {}
                MessageView::StateChanged(sc) => {
                    if sc
                        .src()
                        .map(|s| s == p.upcast_ref::<gstreamer::Object>())
                        .unwrap_or(false)
                        && sc.current() == gstreamer::State::Playing
                    {
                        self.status = ChannelStatus::Running;
                    }
                }
                MessageView::Warning(w) => {
                    let text = format!("{} ({})", w.error(), w.debug().unwrap_or_default());
                    tracing::warn!(channel = self.id, %text, "gst warning");
                }
                MessageView::Element(el) => {
                    if let Some(s) = el.structure() {
                        if s.name() == "level" {
                            Self::apply_level_structure(&mut self.audio_peaks, s);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn apply_level_structure(peaks: &mut [f64; 8], s: &gstreamer::StructureRef) {
        // `peak` is a GValueArray of dBFS doubles (already converted by `level`).
        let Ok(arr) = s.get::<gstreamer::glib::ValueArray>("peak") else {
            return;
        };
        for (i, val) in arr.iter().enumerate().take(8) {
            let Ok(db) = val.get::<f64>() else {
                continue;
            };
            peaks[i] = if db.is_finite() { db.max(-90.0) } else { -90.0 };
        }
    }

    fn read_live_format(&self) -> Option<InputFormat> {
        let p = self.pipeline.as_ref()?;
        let dl = p.by_name("dlsrc")?;
        let pad = dl.static_pad("src")?;
        let caps = pad.current_caps()?;
        format_from_caps(&caps)
    }

    fn adapt_to_mode(&mut self, new_mode: &str) -> Result<()> {
        self.last_adapt = Some(std::time::Instant::now());
        self.locked_mode = new_mode.to_string();
        self.relaunch_preserving_branches(new_mode)
    }

    pub fn snapshot(&self, nvenc_slots_used: usize) -> ChannelSnapshot {
        ChannelSnapshot {
            id: self.id,
            name: self.name.clone(),
            status: self.status,
            encode_preset: self.encode_preset_id.clone(),
            video_bitrate_kbps: self.encode_bitrate.kbps(),
            srt_bitrate_kbps: self.srt_bitrate.as_ref().and_then(|m| m.kbps()),
            recording: self.recording,
            srt: self.srt,
            recording_path: self.recording_path.clone(),
            srt_url: self.srt_url.clone(),
            last_error: self.last_error.clone(),
            nvenc_slots_used,
            configured_mode: self.configured_mode.clone(),
            locked_mode: if self.locked_mode.is_empty() {
                None
            } else {
                Some(self.locked_mode.clone())
            },
            input_format: self.detected.as_ref().map(|f| f.summary()),
            audio_peaks: Some(self.audio_peaks.to_vec()),
        }
    }
}
