//! Pipeline abstractions: capture, record, SRT, preview, playout.

mod describe;
mod preview_sig;
mod signal_format;
mod state;
mod traits;

pub use describe::*;
pub use preview_sig::*;
pub use signal_format::*;
pub use state::*;
pub use traits::*;

#[cfg(feature = "gst")]
pub mod gst;

pub mod mock;

#[cfg(test)]
#[path = "mock/tests.rs"]
mod mock_tests;

use std::sync::Arc;

use roc_config::Config;

/// Build the runtime pipeline backend (GStreamer when feature enabled, else mock).
pub fn create_backend(cfg: &Config) -> Arc<dyn PipelineBackend> {
    #[cfg(feature = "gst")]
    {
        match gst::GstBackend::new() {
            Ok(b) => Arc::new(b.with_nvenc_limit(cfg.max_nvenc_sessions)),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "GStreamer init failed — falling back to mock backend"
                );
                Arc::new(mock::MockBackend::new(cfg.max_nvenc_sessions))
            }
        }
    }
    #[cfg(not(feature = "gst"))]
    {
        Arc::new(mock::MockBackend::new(cfg.max_nvenc_sessions))
    }
}
