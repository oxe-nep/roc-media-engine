//! Per-channel GStreamer capture graph with dynamic REC/SRT branches.

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
    preset: EncodePreset,
    udp_egress: Option<String>,
    pipeline: Option<gstreamer::Pipeline>,
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
            preset: preset.clone(),
            udp_egress: ch.udp_egress.clone(),
            pipeline: None,
        })
    }

    pub fn update_config(&mut self, ch: &ChannelConfig, preset: &EncodePreset) {
        self.name = ch.name.clone();
        self.device = ch.device.clone();
        self.preset = preset.clone();
        self.encode_preset_label = preset.label.clone();
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
        // Dynamic branch attach is complex; for MVP we restart graph with record_path.
        // Encode-once tee keeps a single NVENC session.
        let srt = self.srt_url.clone().filter(|_| self.srt);
        self.relaunch(Some(path.to_string()), srt)?;
        self.recording = true;
        self.recording_path = Some(path.to_string());
        Ok(())
    }

    pub fn stop_recording(&mut self) -> Result<()> {
        if !self.recording {
            return Ok(());
        }
        let srt = self.srt_url.clone().filter(|_| self.srt);
        self.relaunch(None, srt)?;
        self.recording = false;
        self.recording_path = None;
        Ok(())
    }

    pub fn start_srt(&mut self, url: &str) -> Result<()> {
        if self.pipeline.is_none() {
            bail!("capture not running");
        }
        self.srt_url = Some(url.to_string());
        let rec = self.recording_path.clone();
        self.relaunch(rec, Some(url.to_string()))?;
        self.srt = true;
        Ok(())
    }

    pub fn stop_srt(&mut self) -> Result<()> {
        if !self.srt {
            return Ok(());
        }
        let rec = self.recording_path.clone();
        self.relaunch(rec, None)?;
        self.srt = false;
        Ok(())
    }

    fn relaunch(&mut self, record_path: Option<String>, srt_url: Option<String>) -> Result<()> {
        if let Some(p) = self.pipeline.take() {
            let _ = p.set_state(gstreamer::State::Null);
        }
        let launch = build_capture_encode_once_launch(&CaptureLaunchOpts {
            device: self.device.clone(),
            preset: self.preset.clone(),
            preview_path: Some(format!("/tmp/roc-ch{}-preview.ts", self.id)),
            record_path,
            srt_url,
            udp_egress: self.udp_egress.clone(),
            with_tee_preview: true,
        });
        tracing::info!(channel = self.id, %launch, "relaunch capture");
        let pipeline = gstreamer::parse::launch(&launch)
            .context("parse relaunch")?
            .downcast::<gstreamer::Pipeline>()
            .map_err(|_| anyhow!("relaunch did not yield Pipeline"))?;
        pipeline
            .set_state(gstreamer::State::Playing)
            .context("relaunch PLAYING")?;
        self.pipeline = Some(pipeline);
        self.status = ChannelStatus::Waiting;
        Ok(())
    }

    pub fn poll_bus(&mut self) {
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
                MessageView::Eos(_) => {
                    self.status = ChannelStatus::Stopped;
                }
                MessageView::StateChanged(sc) => {
                    if sc.src().map(|s| s == p.upcast_ref::<gstreamer::Object>()).unwrap_or(false)
                        && sc.current() == gstreamer::State::Playing
                    {
                        self.status = ChannelStatus::Running;
                    }
                }
                MessageView::Warning(w) => {
                    let text = format!("{} ({})", w.error(), w.debug().unwrap_or_default());
                    if text.to_lowercase().contains("signal")
                        || text.to_lowercase().contains("no input")
                    {
                        self.status = ChannelStatus::Waiting;
                    }
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
