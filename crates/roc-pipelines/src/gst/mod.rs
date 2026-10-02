//! GStreamer-backed pipelines (in-process, no FFmpeg child processes).

mod capture;
mod probe;

pub use crate::describe::{
    build_capture_launch, build_playout_launch, build_spike_tee_launch, CaptureLaunchOpts,
    PlayoutLaunchOpts,
};
pub use probe::probe_gst_devices;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{anyhow, bail, Context, Result};
use parking_lot::Mutex;
use roc_config::{ChannelConfig, EncodePreset, PlayoutClientConfig};
use roc_devices::DeviceProbeReport;

use crate::{
    ChannelSnapshot, ChannelStatus, PipelineBackend, PlayoutSnapshot, WorkflowKind,
    WorkflowSnapshot,
};

use self::capture::ChannelPipeline;

pub struct GstBackend {
    max_nvenc: usize,
    nvenc_used: AtomicUsize,
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
    last_error: Option<String>,
    pipeline: Option<gstreamer::Pipeline>,
}

impl GstBackend {
    pub fn new() -> Result<Self> {
        gstreamer::init().context("gstreamer::init")?;
        Ok(Self {
            max_nvenc: 8,
            nvenc_used: AtomicUsize::new(0),
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

    fn ensure_channel(&self, ch: &ChannelConfig, preset: &EncodePreset) -> Result<()> {
        self.configs.lock().insert(ch.id, ch.clone());
        self.presets.lock().insert(ch.id, preset.clone());
        let mut map = self.channels.lock();
        if !map.contains_key(&ch.id) {
            map.insert(ch.id, ChannelPipeline::new(ch, preset)?);
        } else if let Some(p) = map.get_mut(&ch.id) {
            p.update_config(ch, preset);
        }
        Ok(())
    }

    fn start_capture(&self, channel_id: u32) -> Result<()> {
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
        let mut map = self.channels.lock();
        let pipe = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        pipe.start_recording(path)
    }

    fn stop_recording(&self, channel_id: u32) -> Result<()> {
        let mut map = self.channels.lock();
        let pipe = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        pipe.stop_recording()
    }

    fn start_srt(&self, channel_id: u32, url: &str) -> Result<()> {
        let mut map = self.channels.lock();
        let pipe = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        pipe.start_srt(url)
    }

    fn stop_srt(&self, channel_id: u32) -> Result<()> {
        let mut map = self.channels.lock();
        let pipe = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        pipe.stop_srt()
    }

    fn channel_snapshot(&self, channel_id: u32) -> Result<ChannelSnapshot> {
        let mut map = self.channels.lock();
        let pipe = map
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow!("channel {channel_id} not registered"))?;
        pipe.poll_bus();
        Ok(pipe.snapshot(self.nvenc_used.load(Ordering::SeqCst)))
    }

    fn list_channels(&self) -> Vec<ChannelSnapshot> {
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

        let launch = build_playout_launch(&PlayoutLaunchOpts {
            source: source.to_string(),
            device: client.device.clone(),
            format_code: client.format_code.clone().unwrap_or_else(|| "Hp50".into()),
        });
        tracing::info!(%launch, client_id = %client.id, "starting playout pipeline");
        let pipeline = gstreamer::parse::launch(&launch)
            .with_context(|| format!("parse playout launch for {}", client.id))?
            .downcast::<gstreamer::Pipeline>()
            .map_err(|_| anyhow!("playout launch did not yield a Pipeline"))?;
        pipeline
            .set_state(gstreamer::State::Playing)
            .context("playout PLAYING")?;

        let mut map = self.playout.lock();
        if let Some(old) = map.get_mut(&client.id) {
            if let Some(p) = old.pipeline.take() {
                let _ = p.set_state(gstreamer::State::Null);
            }
        }
        map.insert(
            client.id.clone(),
            PlayoutRuntime {
                name: client.name.clone(),
                device: client.device.clone(),
                status: ChannelStatus::Running,
                source: Some(source.to_string()),
                last_error: None,
                pipeline: Some(pipeline),
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
        }
        Ok(())
    }

    fn list_playout(&self) -> Vec<PlayoutSnapshot> {
        let map = self.playout.lock();
        let mut out: Vec<_> = map
            .iter()
            .map(|(id, p)| PlayoutSnapshot {
                id: id.clone(),
                name: p.name.clone(),
                status: p.status,
                device: p.device.clone(),
                source: p.source.clone(),
                last_error: p.last_error.clone(),
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
