//! Fas 0 spike: print / optionally run DeckLink → NVENC → file (+ tee preview).

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Result};
use clap::Parser;
use roc_pipelines::build_spike_tee_launch;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "spike-decklink-nvenc")]
struct Args {
    /// DeckLink device name
    #[arg(long, default_value = "DeckLink IP 100G (1)")]
    device: String,

    /// Output fragmented MP4 path
    #[arg(long, default_value = "/tmp/roc-spike.mp4")]
    output: PathBuf,

    /// Also tee a low-bitrate preview TS beside the MP4
    #[arg(long)]
    preview: bool,

    /// Seconds to run (0 = print launch string only)
    #[arg(long, default_value_t = 0)]
    duration_secs: u64,

    /// Force mock backend even if GStreamer is available
    #[arg(long)]
    mock: bool,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let args = Args::parse();
    let launch = build_spike_tee_launch(
        &args.device,
        &args.output.to_string_lossy(),
        args.preview,
    );

    println!("=== Fas 0 spike launch ===");
    println!("{launch}");
    println!();
    println!("Manual (capture host):");
    println!("  gst-launch-1.0 -e {launch}");
    println!();

    if args.duration_secs == 0 {
        println!("duration_secs=0 → launch string only (go/no-go: run with --duration-secs 1800 on host)");
        return Ok(());
    }

    if args.mock {
        let cfg = roc_config::Config::example();
        let backend = roc_pipelines::mock::MockBackend::new(cfg.max_nvenc_sessions);
        let ch = &cfg.channels[0];
        let preset = cfg.preset_for_channel(ch)?;
        use roc_pipelines::PipelineBackend;
        backend.ensure_channel(ch, preset)?;
        backend.start_capture(ch.id)?;
        backend.start_recording(ch.id, &args.output.to_string_lossy())?;
        println!("mock running {}s …", args.duration_secs);
        std::thread::sleep(Duration::from_secs(args.duration_secs));
        backend.stop_recording(ch.id)?;
        backend.stop_capture(ch.id)?;
        println!("mock spike done");
        return Ok(());
    }

    #[cfg(feature = "gst")]
    {
        return run_gst(&args, &launch);
    }

    #[cfg(not(feature = "gst"))]
    {
        bail!("built without `gst` feature — use --mock or build with --features gst on capture host");
    }
}

#[cfg(feature = "gst")]
fn run_gst(args: &Args, launch: &str) -> Result<()> {
    use anyhow::Context;
    use gstreamer::prelude::*;

    gstreamer::init().context("gstreamer init")?;
    let pipeline = gstreamer::parse::launch(launch)
        .context("parse spike launch")?
        .downcast::<gstreamer::Pipeline>()
        .map_err(|_| anyhow::anyhow!("not a Pipeline"))?;

    pipeline
        .set_state(gstreamer::State::Playing)
        .context("PLAYING")?;
    tracing::info!(secs = args.duration_secs, "spike running");

    let bus = pipeline.bus().context("bus")?;
    let deadline = std::time::Instant::now() + Duration::from_secs(args.duration_secs);
    while std::time::Instant::now() < deadline {
        if let Some(msg) = bus.timed_pop(gstreamer::ClockTime::from_mseconds(200)) {
            use gstreamer::MessageView;
            match msg.view() {
                MessageView::Error(e) => {
                    let _ = pipeline.set_state(gstreamer::State::Null);
                    bail!("gst error: {} ({})", e.error(), e.debug().unwrap_or_default());
                }
                MessageView::Eos(_) => break,
                _ => {}
            }
        }
    }

    let _ = pipeline.send_event(gstreamer::event::Eos::new());
    let _ = bus.timed_pop_filtered(
        gstreamer::ClockTime::from_seconds(5),
        &[gstreamer::MessageType::Eos, gstreamer::MessageType::Error],
    );
    let _ = pipeline.set_state(gstreamer::State::Null);
    println!("spike finished → {}", args.output.display());
    Ok(())
}
