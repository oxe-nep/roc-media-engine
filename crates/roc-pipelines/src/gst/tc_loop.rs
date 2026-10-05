//! TC burn-in pipeline helpers (UDP external clock + textoverlay updates).

use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use gstreamer::prelude::*;

/// Spawn a UDP listener that writes normalized timecode into `textoverlay` `text`.
pub fn spawn_external_tc_updater(
    pipeline: &gstreamer::Pipeline,
    udp_port: u16,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let overlay = pipeline
        .by_name("tc_text")
        .context("textoverlay tc_text missing (external TC)")?;
    let sock = UdpSocket::bind(("0.0.0.0", udp_port))
        .with_context(|| format!("bind TC UDP :{udp_port}"))?;
    sock.set_read_timeout(Some(Duration::from_millis(250)))
        .context("TC UDP read timeout")?;
    thread::Builder::new()
        .name(format!("tc-udp-{udp_port}"))
        .spawn(move || {
            let mut buf = [0u8; 256];
            while !stop.load(Ordering::SeqCst) {
                match sock.recv_from(&mut buf) {
                    Ok((n, _)) if n > 0 => {
                        let raw = String::from_utf8_lossy(&buf[..n]);
                        if let Some(tc) = normalize_timecode(raw.trim()) {
                            overlay.set_property("text", &tc);
                        }
                    }
                    _ => {}
                }
            }
        })
        .context("spawn TC UDP thread")?;
    Ok(())
}

/// Accept `HH:MM:SS` or `HH:MM:SS:FF` / `;` / `.` frame separators.
pub fn normalize_timecode(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let parts: Vec<&str> = s.split(|c| c == ':' || c == ';' || c == '.').collect();
    if parts.len() < 3 || parts.len() > 4 {
        return None;
    }
    let h: u32 = parts[0].parse().ok()?;
    let m: u32 = parts[1].parse().ok()?;
    let sec: u32 = parts[2].parse().ok()?;
    if h > 99 || m > 59 || sec > 59 {
        return None;
    }
    if parts.len() == 4 {
        let f: u32 = parts[3].parse().ok()?;
        if f > 99 {
            return None;
        }
        return Some(format!("{h:02}:{m:02}:{sec:02}:{f:02}"));
    }
    Some(format!("{h:02}:{m:02}:{sec:02}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_tod_and_frames() {
        assert_eq!(normalize_timecode("9:05:01").as_deref(), Some("09:05:01"));
        assert_eq!(
            normalize_timecode("01:02:03:04").as_deref(),
            Some("01:02:03:04")
        );
        assert_eq!(
            normalize_timecode("01:02:03;12").as_deref(),
            Some("01:02:03:12")
        );
        assert!(normalize_timecode("nope").is_none());
    }
}
