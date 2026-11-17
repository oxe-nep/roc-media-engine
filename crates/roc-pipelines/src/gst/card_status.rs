//! DeckLink card status via the SDK (`IDeckLinkStatus`), read-only.
//!
//! The card reports the format actually arriving on an input even while our
//! capture holds that input locked to a different mode (where GStreamer only
//! sees `signal=false`). Built only when the SDK headers were found at build
//! time (`cfg(roc_decklink_sdk)`); otherwise [`query`] always returns `None`
//! and the engine falls back to its probe-based re-detection.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardStatus {
    /// Card is locked to a valid input signal.
    pub locked: bool,
    /// Format detected on the wire (GStreamer mode name), if the card reports one.
    pub detected_mode: Option<&'static str>,
    /// Mode the input is currently configured to (what our capture locked).
    pub current_mode: Option<&'static str>,
}

#[cfg(roc_decklink_sdk)]
mod ffi {
    #[repr(C)]
    #[derive(Default)]
    pub struct RocDlStatus {
        pub available: i32,
        pub locked: i32,
        pub detected_mode: u32,
        pub current_mode: u32,
    }

    extern "C" {
        pub fn roc_dl_query(index: u32, out: *mut RocDlStatus) -> i32;
    }
}

/// Status for DeckLink `device-number` `index`; `None` when unavailable.
pub fn query(index: u32) -> Option<CardStatus> {
    #[cfg(roc_decklink_sdk)]
    {
        use crate::signal_format::gst_mode_from_bmd_code;
        let mut s = ffi::RocDlStatus::default();
        // SAFETY: `s` is a valid, exclusively borrowed #[repr(C)] struct matching
        // `roc_dl_status` in native/dl_status.cpp; the callee only writes to it.
        let rc = unsafe { ffi::roc_dl_query(index, &mut s) };
        if rc != 0 || s.available == 0 {
            return None;
        }
        Some(CardStatus {
            locked: s.locked != 0,
            detected_mode: gst_mode_from_bmd_code(s.detected_mode),
            current_mode: gst_mode_from_bmd_code(s.current_mode),
        })
    }
    #[cfg(not(roc_decklink_sdk))]
    {
        let _ = index;
        None
    }
}
