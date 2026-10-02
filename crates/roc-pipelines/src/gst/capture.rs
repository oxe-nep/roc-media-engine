//! Per-channel GStreamer capture graph with dynamic REC/SRT branches (no full relaunch).

use anyhow::{anyhow, bail, Context, Result};
use gstreamer::prelude::*;
use roc_config::{ChannelConfig, EncodePreset};

use crate::describe::{build_capture_encode_once_launch, CaptureLaunchOpts};
use crate::signal_format::{format_from_caps, is_auto_mode, probe_input_format, InputFormat};
use crate::{ChannelSnapshot, ChannelStatus};

pub struct ChannelPipeline {
    pub id: u32,
    pub name: String,
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
}

struct Branch {
    tee_pad: gstreamer::Pad,
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
        Ok(Self {
            id: ch.id,
            name: ch.name.clone(),
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
        })
    }

    pub fn update_config(&mut self, ch: &ChannelConfig, preset: &EncodePreset) {
        self.name = ch.name.clone();
        self.device = ch.device.clone();
        self.configured_mode = ch.mode.clone().unwrap_or_else(|| "auto".into());
        self.preset = preset.clone();
        self.encode_preset_label = preset.label.clone();
        self.parse_element = parse_element_for_codec(&preset.video_codec).to_string();
        self.udp_egress = ch.udp_egress.clone();
        if self.srt_url.is_none() {
            self.srt_url = ch.srt_url.clone();
        }
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
        let launch = build_capture_encode_once_launch(&CaptureLaunchOpts {
            device: self.device.clone(),
            mode: locked.to_string(),
            preset: self.preset.clone(),
            preview_path: Some(format!("/tmp/roc-ch{}-preview.jpg", self.id)),
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

        let queue = gstreamer::ElementFactory::make("queue")
            .name(format!("q_rec_{}", self.id))
            .build()
            .context("queue")?;
        let parse = gstreamer::ElementFactory::make(&self.parse_element)
            .name(format!("parse_rec_{}", self.id))
            .build()
            .with_context(|| format!("make {}", self.parse_element))?;
        let mux = gstreamer::ElementFactory::make("mp4mux")
            .name(format!("mux_rec_{}", self.id))
            .property("fragment-duration", 1000u32)
            .build()
            .context("mp4mux")?;
        let sink = gstreamer::ElementFactory::make("filesink")
            .name(format!("fs_rec_{}", self.id))
            .property("location", path)
            .property("sync", false)
            .build()
            .context("filesink")?;

        pipeline.add_many([&queue, &parse, &mux, &sink])?;
        gstreamer::Element::link_many([&queue, &parse, &mux, &sink])
            .context("link record branch")?;

        let tee_pad = tee
            .request_pad_simple("src_%u")
            .ok_or_else(|| anyhow!("tee request_pad failed"))?;
        let sink_pad = queue
            .static_pad("sink")
            .ok_or_else(|| anyhow!("queue sink pad"))?;
        tee_pad
            .link(&sink_pad)
            .context("link tee → record queue")?;

        for el in [&queue, &parse, &mux, &sink] {
            el.sync_state_with_parent()
                .context("sync_state_with_parent record")?;
        }

        tracing::info!(channel = self.id, %path, "attached record branch (no relaunch)");
        self.rec_branch = Some(Branch {
            tee_pad,
            elements: vec![queue, parse, mux, sink],
        });
        Ok(())
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
        if let Some(qpad) = branch.elements[0].static_pad("sink") {
            let _ = branch.tee_pad.unlink(&qpad);
        }
        tee.release_request_pad(&branch.tee_pad);

        // 2) Optional EOS for a cleaner mp4 footer. Do NOT wait on the pipeline bus:
        //    that raced under multi-channel stop and held the global lock for seconds.
        if finalize {
            if let Some(queue) = branch.elements.first() {
                let _ = queue.send_event(gstreamer::event::Eos::new());
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
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

    fn attach_srt(&mut self, url: &str) -> Result<()> {
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow!("no pipeline"))?
            .clone();
        let tee = self.encoded_tee()?;

        let queue = gstreamer::ElementFactory::make("queue")
            .name(format!("q_srt_{}", self.id))
            .build()
            .context("queue")?;
        let parse = gstreamer::ElementFactory::make(&self.parse_element)
            .name(format!("parse_srt_{}", self.id))
            .build()
            .with_context(|| format!("make {}", self.parse_element))?;
        let mux = gstreamer::ElementFactory::make("mpegtsmux")
            .name(format!("mux_srt_{}", self.id))
            .property("alignment", 7i32)
            .build()
            .context("mpegtsmux")?;
        let sink = gstreamer::ElementFactory::make("srtsink")
            .name(format!("srt_sink_{}", self.id))
            .property("uri", url)
            .property("wait-for-connection", false)
            .build()
            .context("srtsink")?;

        pipeline.add_many([&queue, &parse, &mux, &sink])?;
        gstreamer::Element::link_many([&queue, &parse, &mux, &sink])
            .context("link srt branch")?;

        let tee_pad = tee
            .request_pad_simple("src_%u")
            .ok_or_else(|| anyhow!("tee request_pad failed"))?;
        let sink_pad = queue
            .static_pad("sink")
            .ok_or_else(|| anyhow!("queue sink pad"))?;
        tee_pad.link(&sink_pad).context("link tee → srt queue")?;

        for el in [&queue, &parse, &mux, &sink] {
            el.sync_state_with_parent()
                .context("sync_state_with_parent srt")?;
        }

        tracing::info!(channel = self.id, %url, "attached SRT branch (no relaunch)");
        self.srt_branch = Some(Branch {
            tee_pad,
            elements: vec![queue, parse, mux, sink],
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

        if let Some(qpad) = branch.elements[0].static_pad("sink") {
            let _ = branch.tee_pad.unlink(&qpad);
        }
        tee.release_request_pad(&branch.tee_pad);

        for el in branch.elements.iter().rev() {
            let _ = el.set_state(gstreamer::State::Null);
        }
        for el in &branch.elements {
            let _ = pipeline.remove(el);
        }
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
                _ => {}
            }
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
        self.locked_mode = new_mode.to_string();
        self.launch_locked(new_mode)?;

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

    pub fn snapshot(&self, nvenc_slots_used: usize) -> ChannelSnapshot {
        ChannelSnapshot {
            id: self.id,
            name: self.name.clone(),
            status: self.status,
            encode_preset: self.encode_preset_label.clone(),
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
        }
    }
}
