//! Per-channel GStreamer capture graph with dynamic REC/SRT branches (no full relaunch).

use anyhow::{anyhow, bail, Context, Result};
use gstreamer::prelude::*;
use roc_config::{ChannelConfig, EncodePreset};

use crate::describe::{build_capture_encode_once_launch, CaptureLaunchOpts};
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
    mode: String,
    preset: EncodePreset,
    udp_egress: Option<String>,
    parse_element: String,
    pipeline: Option<gstreamer::Pipeline>,
    rec_branch: Option<Branch>,
    srt_branch: Option<Branch>,
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
            mode: ch.mode.clone().unwrap_or_else(|| "1080p50".into()),
            parse_element: parse_element_for_codec(&preset.video_codec).to_string(),
            preset: preset.clone(),
            udp_egress: ch.udp_egress.clone(),
            pipeline: None,
            rec_branch: None,
            srt_branch: None,
        })
    }

    pub fn update_config(&mut self, ch: &ChannelConfig, preset: &EncodePreset) {
        self.name = ch.name.clone();
        self.device = ch.device.clone();
        self.mode = ch.mode.clone().unwrap_or_else(|| "1080p50".into());
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
        let launch = build_capture_encode_once_launch(&CaptureLaunchOpts {
            device: self.device.clone(),
            mode: self.mode.clone(),
            preset: self.preset.clone(),
            preview_path: Some(format!("/tmp/roc-ch{}-preview.ts", self.id)),
            record_path: None,
            srt_url: None,
            udp_egress: self.udp_egress.clone(),
            with_tee_preview: true,
        });
        tracing::info!(channel = self.id, %launch, "capture pipeline");
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

        if finalize {
            // EOS only this branch so mp4mux finalizes the file.
            if let Some(queue) = branch.elements.first() {
                if let Some(pad) = queue.static_pad("sink") {
                    let _ = pad.send_event(gstreamer::event::Eos::new());
                }
            }
            // Brief wait for mux to flush.
            let bus = pipeline.bus().context("bus")?;
            let _ = bus.timed_pop_filtered(
                gstreamer::ClockTime::from_mseconds(1500),
                &[gstreamer::MessageType::Eos, gstreamer::MessageType::Error],
            );
        }

        for el in &branch.elements {
            let _ = el.set_state(gstreamer::State::Null);
        }
        if let Some(qpad) = branch.elements[0].static_pad("sink") {
            let _ = branch.tee_pad.unlink(&qpad);
        }
        tee.release_request_pad(&branch.tee_pad);
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

        for el in &branch.elements {
            let _ = el.set_state(gstreamer::State::Null);
        }
        if let Some(qpad) = branch.elements[0].static_pad("sink") {
            let _ = branch.tee_pad.unlink(&qpad);
        }
        tee.release_request_pad(&branch.tee_pad);
        for el in &branch.elements {
            let _ = pipeline.remove(el);
        }
        tracing::info!(channel = self.id, "detached SRT branch");
        Ok(())
    }

    pub fn poll_bus(&mut self) {
        let Some(p) = self.pipeline.as_ref() else {
            return;
        };
        // Authoritative state — don't rely only on catching StateChanged transitions.
        let (_, cur, _) = p.state(gstreamer::ClockTime::ZERO);
        if cur == gstreamer::State::Playing && self.status != ChannelStatus::Error {
            self.status = ChannelStatus::Running;
        }
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
        }
    }
}
