//! Run GStreamer-touching work off the Tokio runtime.
//!
//! Pipeline ops take `gst_op` and may wait on EOS / create-offer; doing that on
//! a worker thread keeps the async reactor responsive for WS meters and HTTP.

/// Execute `f` on Tokio's blocking pool and return its result.
pub async fn run_blocking<T, F>(f: F) -> T
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .expect("gst blocking task joined")
}
