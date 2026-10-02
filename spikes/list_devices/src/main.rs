//! Fas 0: list GStreamer DeckLink / encoder availability.

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
struct Args {
    #[arg(long)]
    mock: bool,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let args = Args::parse();
    let report = if args.mock {
        roc_devices::DeviceProbeReport::empty_mock()
    } else {
        #[cfg(feature = "gst")]
        {
            gstreamer::init()?;
            roc_pipelines::gst::probe_gst_devices()?
        }
        #[cfg(not(feature = "gst"))]
        {
            anyhow::bail!("built without gst — pass --mock or build with --features gst");
        }
    };

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
