//! DeckLink card status via the SDK (`IDeckLinkStatus`), read-only.
//!
//! The card reports the format actually arriving on an input even while our
//! capture holds that input locked to a different mode (where GStreamer only
//! sees `signal=false`). Built only when the SDK headers were found at build
//! time (`cfg(roc_decklink_sdk)`); otherwise [`card_status`] always returns `None`
//! and the engine falls back to its probe-based re-detection.

use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardStatus {
    /// Card is locked to a valid input signal.
    pub locked: bool,
    /// Format detected on the wire (GStreamer mode name), if the card reports one.
    pub detected_mode: Option<&'static str>,
    /// Mode the input is currently configured to (what our capture locked).
    pub current_mode: Option<&'static str>,
}

/// Shared "follow the card's input format" policy for encode capture and TC.
///
/// Guards against relaunch storms (a relaunch loop once OOM-killed the host):
/// - the card must report the same differing mode for `STABLE`;
/// - the same target is not retried within `TARGET_BACKOFF`;
/// - at most `MAX_PER_WINDOW` relaunches per `WINDOW`.
#[derive(Debug, Default)]
pub struct CardFormatFollower {
    last_check: Option<Instant>,
    seen: Option<(&'static str, Instant)>,
    last_target: Option<(&'static str, Instant)>,
    recent: Vec<Instant>,
}


impl CardFormatFollower {
    pub const CHECK_EVERY: Duration = Duration::from_millis(500);
    pub const STABLE: Duration = Duration::from_secs(1);
    pub const TARGET_BACKOFF: Duration = Duration::from_secs(60);
    pub const WINDOW: Duration = Duration::from_secs(60);
    pub const MAX_PER_WINDOW: usize = 4;

    /// Rate limit card reads (meter ticks poll at ~30 Hz). True = read now.
    pub fn due(&mut self, now: Instant) -> bool {
        if self
            .last_check
            .is_some_and(|t| now.duration_since(t) < Self::CHECK_EVERY)
        {
            return false;
        }
        self.last_check = Some(now);
        true
    }

    /// Mode the card reports when it differs from `locked`, once stable and
    /// not throttled. Caller applies its own recording / cooldown rules, then
    /// calls [`Self::committed`] when it actually relaunches.
    pub fn candidate(&mut self, st: &CardStatus, locked: &str, now: Instant) -> Option<&'static str> {
        let detected = match st.detected_mode {
            Some(m) if st.locked && m != locked => m,
            _ => {
                self.seen = None;
                return None;
            }
        };
        match self.seen {
            Some((m, since)) if m == detected => {
                if now.duration_since(since) < Self::STABLE {
                    return None;
                }
            }
            _ => {
                self.seen = Some((detected, now));
                return None;
            }
        }
        if self
            .last_target
            .is_some_and(|(m, t)| m == detected && now.duration_since(t) < Self::TARGET_BACKOFF)
        {
            return None;
        }
        self.recent.retain(|t| now.duration_since(*t) < Self::WINDOW);
        if self.recent.len() >= Self::MAX_PER_WINDOW {
            return None;
        }
        Some(detected)
    }

    pub fn committed(&mut self, mode: &'static str, now: Instant) {
        self.seen = None;
        self.last_target = Some((mode, now));
        self.recent.push(now);
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
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

/// Status for a configured device name (`"DeckLink IP 100G (5)"` / `"5"`).
pub fn card_status_for_device(device: &str) -> Option<CardStatus> {
    card_status(crate::describe::decklink_device_number(device))
}

/// Status for DeckLink `device-number` `index`; `None` when unavailable.
pub fn card_status(index: u32) -> Option<CardStatus> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn locked(mode: &'static str) -> CardStatus {
        CardStatus { locked: true, detected_mode: Some(mode), current_mode: None }
    }

    #[test]
    fn needs_stable_mode_before_adapting() {
        let mut f = CardFormatFollower::default();
        let t0 = Instant::now();
        assert_eq!(f.candidate(&locked("1080i50"), "1080p50", t0), None);
        assert_eq!(f.candidate(&locked("1080i50"), "1080p50", t0 + Duration::from_millis(500)), None);
        assert_eq!(
            f.candidate(&locked("1080i50"), "1080p50", t0 + Duration::from_millis(1100)),
            Some("1080i50")
        );
    }

    #[test]
    fn flicker_resets_stability() {
        let mut f = CardFormatFollower::default();
        let t0 = Instant::now();
        f.candidate(&locked("1080i50"), "1080p50", t0);
        f.candidate(&locked("1080p25"), "1080p50", t0 + Duration::from_millis(900));
        assert_eq!(f.candidate(&locked("1080i50"), "1080p50", t0 + Duration::from_millis(1500)), None);
    }

    #[test]
    fn no_candidate_when_matching_or_unlocked() {
        let mut f = CardFormatFollower::default();
        let t0 = Instant::now();
        let unlocked = CardStatus { locked: false, detected_mode: Some("1080i50"), current_mode: None };
        for i in 0..5 {
            let t = t0 + Duration::from_secs(i);
            assert_eq!(f.candidate(&locked("1080p50"), "1080p50", t), None);
            assert_eq!(f.candidate(&unlocked, "1080p50", t), None);
        }
    }

    #[test]
    fn backs_off_same_target_and_caps_rate() {
        let mut f = CardFormatFollower::default();
        let mut t = Instant::now();
        let adapt = |f: &mut CardFormatFollower, t: &mut Instant, to: &'static str, from: &str| {
            f.candidate(&locked(to), from, *t);
            *t += Duration::from_millis(1100);
            let c = f.candidate(&locked(to), from, *t);
            if let Some(m) = c {
                f.committed(m, *t);
            }
            c
        };
        assert_eq!(adapt(&mut f, &mut t, "1080i50", "1080p50"), Some("1080i50"));
        // Relaunch into i50 did not take (still locked p50): same target backed off.
        assert_eq!(adapt(&mut f, &mut t, "1080i50", "1080p50"), None);
        assert_eq!(adapt(&mut f, &mut t, "1080p50", "1080i50"), Some("1080p50"));
        assert_eq!(adapt(&mut f, &mut t, "720p50", "1080p50"), Some("720p50"));
        assert_eq!(adapt(&mut f, &mut t, "2160p50", "720p50"), Some("2160p50"));
        // 4 relaunches inside the window → capped.
        assert_eq!(adapt(&mut f, &mut t, "1080p25", "2160p50"), None);
        t += CardFormatFollower::WINDOW;
        assert_eq!(adapt(&mut f, &mut t, "1080p25", "2160p50"), Some("1080p25"));
    }
}
