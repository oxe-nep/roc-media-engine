//! Preview WebRTC signaling messages (no GStreamer dependency).

use std::sync::Arc;
use std::sync::mpsc;

use parking_lot::Mutex;

#[derive(Debug, Clone)]
pub enum PreviewSignal {
    Offer { channel: u32, sdp: String },
    Ice {
        channel: u32,
        candidate: String,
        sdp_mline_index: u32,
    },
    Error { channel: u32, message: String },
}

pub type PreviewSignalTx = Arc<Mutex<Option<mpsc::Sender<PreviewSignal>>>>;

pub fn new_preview_signal_tx() -> (PreviewSignalTx, mpsc::Receiver<PreviewSignal>) {
    let (tx, rx) = mpsc::channel();
    (Arc::new(Mutex::new(Some(tx))), rx)
}
