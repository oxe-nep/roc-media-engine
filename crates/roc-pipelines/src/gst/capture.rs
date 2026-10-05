//! Per-channel GStreamer capture graph.
//!
//! REC attaches as a dynamic branch on the encoded tee `e` and is independent
//! of SRT. Program MPEG-TS is muxed once (Hydra-style) to `ts_out` → UDP +
//! `appsink srt_in`. SRT start/stop only arms/disarms appsrc→srtsink on that
//! appsink — no capture relaunch, so an active REC is never torn down.

use anyhow::{anyhow, bail, Context, Result};
use gstreamer::prelude::*;
use roc_config::{ChannelConfig, EncodePreset};
use std::str::FromStr;

use super::bitrate::BitrateMeter;
use crate::describe::{
    aac_stereo_pairs, build_capture_encode_once_launch, stereo_pair_matrix, CaptureLaunchOpts,
};
use crate::signal_format::{format_from_caps, is_auto_mode, probe_input_format, InputFormat};
use crate::{ChannelSnapshot, ChannelStatus};

/// Insert an `identity` that forces a clean TIME segment from 0.
///
/// Late-join REC/SRT sees video PTS in a different domain than live AAC
/// (observed: ~1e15 ns vs ~1e9 ns). Without a segment reset, mp4mux/mpegtsmux
/// remaps each pad independently and lipsync drifts. `single-segment=true`
/// rewrites timestamps onto a fresh segment; pair with audio
/// `min-threshold-time` so AAC is held roughly one NVENC latency behind.
fn make_mux_ts_align(name: &str) -> Result<gstreamer::Element> {
    gstreamer::ElementFactory::make("identity")
        .name(name)
        .property("single-segment", true)
        .property("silent", true)
        .build()
        .context("identity single-segment")
}

fn install_mux_av_sync_log(tag: &str) {
    // Sync is applied via identity + keyframe-gated AAC; this log keeps
    // deploy/verify scripts discoverable.
    tracing::info!(%tag, "mux A/V sync: identity single-segment + keyframe-gated AAC");
}

/// Drop AAC buffers until `gate` is opened (first video keyframe).
/// Installed on the audio queue sink so stale pre-keyframe PCM never buffers.
fn install_av_start_gate(audio_queue: &gstreamer::Element, gate: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    use gstreamer::{PadProbeReturn, PadProbeType};
    use std::sync::atomic::Ordering;

    let Some(pad) = audio_queue.static_pad("sink") else {
        return;
    };
    pad.add_probe(PadProbeType::BUFFER, move |_, _info| {
        if gate.load(Ordering::SeqCst) {
            PadProbeReturn::Ok
        } else {
            PadProbeReturn::Drop
        }
    });
}

/// Open `gate` when the first non-delta (key) video buffer is seen.
/// Also drops leading delta units so identity single-segment starts on the IDR.
fn arm_av_gate_on_keyframe(video_identity: &gstreamer::Element, gate: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    use gstreamer::{PadProbeReturn, PadProbeType};
    use std::sync::atomic::Ordering;

    // Probe the *sink* so single-segment sees the keyframe as its first buffer.
    let Some(pad) = video_identity.static_pad("sink") else {
        return;
    };
    pad.add_probe(PadProbeType::BUFFER, move |_, info| {
        let Some(buf) = info.buffer() else {
            return PadProbeReturn::Ok;
        };
        if buf.flags().contains(gstreamer::BufferFlags::DELTA_UNIT) {
            return PadProbeReturn::Drop;
        }
        gate.store(true, Ordering::SeqCst);
        tracing::info!("mux A/V sync: opened AAC gate on video keyframe");
        PadProbeReturn::Remove
    });
}

/// Wire `appsink srt_in` → gated `appsrc` → `srtsink`.
///
/// Pad-probe Drop was leaking poison BUFFER_LISTs (open-chunk dump was clean A/V
/// while the SRT capture still started with an older AAC-only PMT). Pulling in
/// userspace and only pushing approved buffers matches how we must control the
/// first bytes MediaMTX locks on. HydraSRT avoids this by never remuxing; we
/// still encode, so the gate stays.
fn arm_srt_appsink_gate(
    channel: u32,
    pipeline: &gstreamer::Pipeline,
    srt_uri: &str,
    min_audio_es: usize,
) -> anyhow::Result<()> {
    use gstreamer::prelude::*;
    use gstreamer::{Caps, FlowError, FlowSuccess, Format};
    use gstreamer_app::{AppSink, AppSinkCallbacks, AppSrc};
    use std::str::FromStr;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;

    let appsink_el = pipeline
        .by_name("srt_in")
        .ok_or_else(|| anyhow::anyhow!("appsink srt_in missing"))?;
    let appsink = appsink_el
        .downcast::<AppSink>()
        .map_err(|_| anyhow::anyhow!("srt_in is not AppSink"))?;

    let appsrc_el = gstreamer::ElementFactory::make("appsrc")
        .name(format!("srt_out_{channel}"))
        .build()
        .map_err(|e| anyhow::anyhow!("appsrc: {e}"))?;
    let appsrc = appsrc_el
        .clone()
        .downcast::<AppSrc>()
        .map_err(|_| anyhow::anyhow!("appsrc downcast"))?;
    appsrc.set_format(Format::Bytes);
    appsrc.set_is_live(true);
    appsrc.set_block(false);
    appsrc.set_property("emit-signals", false);
    let _ = appsrc.set_caps(Some(
        &Caps::from_str("video/mpegts,systemstream=(boolean)true")
            .map_err(|e| anyhow::anyhow!("mpegts caps: {e}"))?,
    ));

    let srtsink = gstreamer::ElementFactory::make("srtsink")
        .name(format!("srt_sink_{channel}"))
        .property("uri", srt_uri)
        .property("wait-for-connection", false)
        .property("auto-reconnect", true)
        .property("async", false)
        .property("sync", false)
        .build()
        .map_err(|e| anyhow::anyhow!("srtsink: {e}"))?;

    pipeline
        .add_many([&appsrc_el, &srtsink])
        .map_err(|e| anyhow::anyhow!("add srt appsrc/sink: {e}"))?;
    appsrc_el
        .link(&srtsink)
        .map_err(|e| anyhow::anyhow!("link appsrc→srtsink: {e}"))?;
    appsrc_el
        .sync_state_with_parent()
        .map_err(|e| anyhow::anyhow!("sync appsrc: {e}"))?;
    srtsink
        .sync_state_with_parent()
        .map_err(|e| anyhow::anyhow!("sync srtsink: {e}"))?;

    let min_audio_es = min_audio_es.max(1);
    let opened = Arc::new(AtomicBool::new(false));
    let last_log = Arc::new(AtomicU64::new(0));
    let first_ts = Arc::new(AtomicU64::new(0));
    let appsrc_cb = appsrc.clone();

    appsink.set_callbacks(
        AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = match sink.pull_sample() {
                    Ok(s) => s,
                    Err(_) => return Err(FlowError::Error),
                };
                let Some(buffer) = sample.buffer() else {
                    return Ok(FlowSuccess::Ok);
                };
                let Ok(map) = buffer.map_readable() else {
                    return Ok(FlowSuccess::Ok);
                };
                let chunk = map.as_slice();
                if chunk.is_empty() {
                    return Ok(FlowSuccess::Ok);
                }

                let has_poison = chunk_has_incomplete_pmt(chunk, min_audio_es);
                let first_ok = first_pmt_is_full_av(chunk, min_audio_es);

                if opened.load(Ordering::SeqCst) {
                    if has_poison && !first_ok {
                        return Ok(FlowSuccess::Ok);
                    }
                } else {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let _ = first_ts.compare_exchange(
                        0,
                        now,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    );
                    if !(first_ok && !has_poison && ts_starts_with_pat(chunk)) {
                        let started = first_ts.load(Ordering::Relaxed);
                        let prev = last_log.load(Ordering::Relaxed);
                        if now != prev
                            && last_log
                                .compare_exchange(
                                    prev,
                                    now,
                                    Ordering::Relaxed,
                                    Ordering::Relaxed,
                                )
                                .is_ok()
                        {
                            let (_, types, audio_pid_list) =
                                ts_ready_for_mediamtx(std::slice::from_ref(&chunk.to_vec()), min_audio_es);
                            tracing::warn!(
                                channel,
                                bytes = chunk.len(),
                                ?types,
                                min_audio_es,
                                audio = audio_pid_list.len(),
                                waited_s = now.saturating_sub(started),
                                "SRT appsink waiting for PAT + full A/V PMT"
                            );
                        }
                        return Ok(FlowSuccess::Ok);
                    }
                    if opened
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                    {
                        let dump = format!("/tmp/srt-gate-open-ch{channel}.ts");
                        let _ = std::fs::write(&dump, chunk);
                        let (_, types, audio_pid_list) =
                            ts_ready_for_mediamtx(std::slice::from_ref(&chunk.to_vec()), min_audio_es);
                        tracing::info!(
                            channel,
                            ?types,
                            ?audio_pid_list,
                            bytes = chunk.len(),
                            %dump,
                            "SRT appsink gate open — pushing first clean buffer"
                        );
                    }
                }

                let out = buffer.copy();
                match appsrc_cb.push_buffer(out) {
                    Ok(_) => Ok(FlowSuccess::Ok),
                    Err(_) => Err(FlowError::Flushing),
                }
            })
            .build(),
    );

    tracing::info!(channel, %srt_uri, "SRT appsink↔appsrc gate armed");
    Ok(())
}

/// Drop appsrc→srtsink and clear appsink callbacks so TS keeps flowing to UDP
/// while SRT is idle (appsink drops with max-buffers).
fn disarm_srt_appsink_gate(channel: u32, pipeline: &gstreamer::Pipeline) {
    use gstreamer::prelude::*;
    use gstreamer_app::{AppSink, AppSinkCallbacks};

    for name in [
        format!("srt_sink_{channel}"),
        format!("srt_out_{channel}"),
    ] {
        if let Some(el) = pipeline.by_name(&name) {
            let _ = el.set_state(gstreamer::State::Null);
            if let Some(pad) = el.static_pad("sink") {
                if let Some(peer) = pad.peer() {
                    let _ = peer.unlink(&pad);
                }
            }
            if let Some(pad) = el.static_pad("src") {
                if let Some(peer) = pad.peer() {
                    let _ = pad.unlink(&peer);
                }
            }
            let _ = pipeline.remove(&el);
        }
    }
    if let Some(el) = pipeline.by_name("srt_in") {
        if let Ok(appsink) = el.downcast::<AppSink>() {
            appsink.set_callbacks(AppSinkCallbacks::builder().build());
        }
    }
}

/// Collect PMT PIDs declared in PAT (PID 0). Avoids false "PMT" matches inside
/// video PES when the rolling window loses 188-byte alignment.
fn pat_pmt_pids(ts: &[u8]) -> Vec<u16> {
    let mut pids = Vec::new();
    let mut i = 0;
    while i + 188 <= ts.len() {
        if ts[i] != 0x47 {
            i += 1;
            continue;
        }
        let pkt = &ts[i..i + 188];
        let pid = (((pkt[1] & 0x1f) as u16) << 8) | pkt[2] as u16;
        if pid != 0 || pkt[1] & 0x40 == 0 {
            i += 188;
            continue;
        }
        let afc = (pkt[3] >> 4) & 0x3;
        let mut off = 4usize;
        if afc == 2 || afc == 3 {
            off = 5 + pkt[4] as usize;
        }
        if off >= 187 {
            i += 188;
            continue;
        }
        let ptr = pkt[off] as usize;
        let p = off + 1 + ptr;
        if p + 8 >= 188 || pkt[p] != 0x00 {
            i += 188;
            continue;
        }
        let sl = (((pkt[p + 1] & 0x0f) as usize) << 8) | pkt[p + 2] as usize;
        let mut q = p + 8;
        let end = (p + 3 + sl).saturating_sub(4).min(188);
        while q + 4 <= end {
            let prog = ((pkt[q] as u16) << 8) | pkt[q + 1] as u16;
            let ppid = (((pkt[q + 2] & 0x1f) as u16) << 8) | pkt[q + 3] as u16;
            if prog != 0 && !pids.contains(&ppid) {
                pids.push(ppid);
            }
            q += 4;
        }
        i += 188;
    }
    pids
}

/// One PMT elementary stream: (stream_type, elementary_PID).
/// Only parses PIDs listed in the PAT — reassembles multi-packet sections.
fn pmt_sections(ts: &[u8]) -> Vec<Vec<(u8, u16)>> {
    let pmt_pids = pat_pmt_pids(ts);
    if pmt_pids.is_empty() {
        return Vec::new();
    }
    let mut sections = Vec::new();
    let mut i = 0;
    while i + 188 <= ts.len() {
        if ts[i] != 0x47 {
            i += 1;
            continue;
        }
        let pkt = &ts[i..i + 188];
        let pid = (((pkt[1] & 0x1f) as u16) << 8) | pkt[2] as u16;
        if !pmt_pids.contains(&pid) || pkt[1] & 0x40 == 0 {
            i += 188;
            continue;
        }
        let afc = (pkt[3] >> 4) & 0x3;
        let mut off = 4usize;
        if afc == 2 || afc == 3 {
            let afl = pkt[4] as usize;
            off = 5 + afl;
        }
        if off >= 187 {
            i += 188;
            continue;
        }
        let ptr = pkt[off] as usize;
        let p = off + 1 + ptr;
        if p + 12 >= 188 || pkt[p] != 0x02 {
            i += 188;
            continue;
        }
        let sl = (((pkt[p + 1] & 0x0f) as usize) << 8) | pkt[p + 2] as usize;
        let need = 3 + sl;
        if need < 16 || need > 1024 {
            i += 188;
            continue;
        }
        let mut buf = pkt[p..].to_vec();
        let mut j = i + 188;
        while buf.len() < need && j + 188 <= ts.len() {
            if ts[j] != 0x47 {
                j += 1;
                continue;
            }
            let cont_pid = (((ts[j + 1] & 0x1f) as u16) << 8) | ts[j + 2] as u16;
            if cont_pid != pid {
                j += 188;
                continue;
            }
            if ts[j + 1] & 0x40 != 0 {
                break;
            }
            let afc2 = (ts[j + 3] >> 4) & 0x3;
            let mut o = 4usize;
            if afc2 == 2 || afc2 == 3 {
                o = 5 + ts[j + 4] as usize;
            }
            if o < 188 {
                buf.extend_from_slice(&ts[j + o..j + 188]);
            }
            j += 188;
        }
        if buf.len() < need {
            i += 188;
            continue;
        }
        buf.truncate(need);
        let pil = (((buf[10] & 0x0f) as usize) << 8) | buf[11] as usize;
        let mut q = 12 + pil;
        let end = need.saturating_sub(4);
        let mut found = Vec::new();
        while q + 5 <= end {
            let st = buf[q];
            let epid = (((buf[q + 1] & 0x1f) as u16) << 8) | buf[q + 2] as u16;
            let esil = (((buf[q + 3] & 0x0f) as usize) << 8) | buf[q + 4] as usize;
            if esil > 64 || q + 5 + esil > end {
                break;
            }
            if epid >= 0x20
                && epid < 0x1fff
                && matches!(st, 0x1b | 0x24 | 0x0f | 0x11)
            {
                found.push((st, epid));
            }
            q += 5 + esil;
        }
        if !found.is_empty() {
            sections.push(found);
        }
        i += 188;
    }
    sections
}

fn pmt_has_av(es: &[(u8, u16)], min_audio: usize) -> bool {
    let video = es.iter().any(|&(t, _)| t == 0x1b || t == 0x24);
    let mut audio_pids = Vec::new();
    for &(t, pid) in es {
        if (t == 0x0f || t == 0x11) && !audio_pids.contains(&pid) {
            audio_pids.push(pid);
        }
    }
    video && audio_pids.len() >= min_audio.max(1)
}

/// True when the payload contains at least one PMT that is not full A/V.
fn chunk_has_incomplete_pmt(ts: &[u8], min_audio: usize) -> bool {
    if pmt_sections(ts)
        .into_iter()
        .any(|sec| !pmt_has_av(&sec, min_audio))
    {
        return true;
    }
    // PMT-only buffers (no PAT in the same alignment=7 burst) are invisible to
    // pat_pmt_pids — scan PUSI packets for table_id 0x02 directly.
    orphan_pmt_is_incomplete(ts, min_audio)
}

/// Parse PUSI packets with table_id=0x02 even when PAT is absent from `ts`.
fn orphan_pmt_is_incomplete(ts: &[u8], min_audio: usize) -> bool {
    let mut i = 0usize;
    while i + 188 <= ts.len() {
        if ts[i] != 0x47 {
            i += 1;
            continue;
        }
        let pkt = &ts[i..i + 188];
        if pkt[1] & 0x40 == 0 {
            i += 188;
            continue;
        }
        let afc = (pkt[3] >> 4) & 0x3;
        let mut off = 4usize;
        if afc == 2 || afc == 3 {
            off = 5 + pkt[4] as usize;
        }
        if off >= 187 {
            i += 188;
            continue;
        }
        let ptr = pkt[off] as usize;
        let p = off + 1 + ptr;
        if p + 12 >= 188 || pkt[p] != 0x02 {
            i += 188;
            continue;
        }
        let secs = pmt_sections_at(ts, i);
        if let Some(sec) = secs.first() {
            if !pmt_has_av(sec, min_audio) {
                return true;
            }
        }
        i += 188;
    }
    false
}

/// Like `pmt_sections` but starts at a known PMT packet offset (no PAT lookup).
fn pmt_sections_at(ts: &[u8], start: usize) -> Vec<Vec<(u8, u16)>> {
    if start + 188 > ts.len() {
        return Vec::new();
    }
    let pkt = &ts[start..start + 188];
    let pid = (((pkt[1] & 0x1f) as u16) << 8) | pkt[2] as u16;
    let afc = (pkt[3] >> 4) & 0x3;
    let mut off = 4usize;
    if afc == 2 || afc == 3 {
        off = 5 + pkt[4] as usize;
    }
    if off >= 187 {
        return Vec::new();
    }
    let ptr = pkt[off] as usize;
    let p = off + 1 + ptr;
    if p + 12 >= 188 || pkt[p] != 0x02 {
        return Vec::new();
    }
    let sl = (((pkt[p + 1] & 0x0f) as usize) << 8) | pkt[p + 2] as usize;
    let need = 3 + sl;
    if need < 16 || need > 1024 {
        return Vec::new();
    }
    let mut buf = pkt[p..].to_vec();
    let mut j = start + 188;
    while buf.len() < need && j + 188 <= ts.len() {
        if ts[j] != 0x47 {
            j += 1;
            continue;
        }
        let cont_pid = (((ts[j + 1] & 0x1f) as u16) << 8) | ts[j + 2] as u16;
        if cont_pid != pid {
            j += 188;
            continue;
        }
        if ts[j + 1] & 0x40 != 0 {
            break;
        }
        let afc2 = (ts[j + 3] >> 4) & 0x3;
        let mut o = 4usize;
        if afc2 == 2 || afc2 == 3 {
            o = 5 + ts[j + 4] as usize;
        }
        if o < 188 {
            buf.extend_from_slice(&ts[j + o..j + 188]);
        }
        j += 188;
    }
    if buf.len() < need {
        return Vec::new();
    }
    buf.truncate(need);
    let pil = (((buf[10] & 0x0f) as usize) << 8) | buf[11] as usize;
    let mut q = 12 + pil;
    let end = need.saturating_sub(4);
    let mut found = Vec::new();
    while q + 5 <= end {
        let st = buf[q];
        let epid = (((buf[q + 1] & 0x1f) as u16) << 8) | buf[q + 2] as u16;
        let esil = (((buf[q + 3] & 0x0f) as usize) << 8) | buf[q + 4] as usize;
        if esil > 64 || q + 5 + esil > end {
            break;
        }
        if epid >= 0x20 && epid < 0x1fff && matches!(st, 0x1b | 0x24 | 0x0f | 0x11) {
            found.push((st, epid));
        }
        q += 5 + esil;
    }
    if found.is_empty() {
        Vec::new()
    } else {
        vec![found]
    }
}

/// True when the first aligned TS packet is a PAT (PID 0, PUSI).
fn ts_starts_with_pat(ts: &[u8]) -> bool {
    let Some(i) = ts.iter().position(|&b| b == 0x47) else {
        return false;
    };
    if i + 188 > ts.len() {
        return false;
    }
    let pkt = &ts[i..i + 188];
    let pid = (((pkt[1] & 0x1f) as u16) << 8) | pkt[2] as u16;
    pid == 0 && pkt[1] & 0x40 != 0
}

/// True when the first PMT in wire order is full A/V (what MediaMTX locks on).
fn first_pmt_is_full_av(ts: &[u8], min_audio: usize) -> bool {
    pmt_sections(ts)
        .into_iter()
        .next()
        .is_some_and(|sec| pmt_has_av(&sec, min_audio))
}

/// Ready when the *latest* PMT in the payload is full A/V.
fn ts_ready_for_mediamtx(
    chunks: &[Vec<u8>],
    min_audio: usize,
) -> (bool, Vec<u8>, Vec<u16>) {
    // mpegtsmux alignment=7 emits BUFFER_LIST; PMT sections often span list
    // elements — parse the concatenated payload, not each buffer alone.
    let mut data = Vec::new();
    for c in chunks {
        data.extend_from_slice(c);
    }
    let mut union = Vec::new();
    let mut best_audio: Vec<u16> = Vec::new();
    let mut last: Option<Vec<(u8, u16)>> = None;
    for sec in pmt_sections(&data) {
        let mut uniq = Vec::new();
        for &(t, pid) in &sec {
            if (t == 0x0f || t == 0x11) && !uniq.contains(&pid) {
                uniq.push(pid);
            }
        }
        if uniq.len() > best_audio.len() {
            best_audio = uniq;
        }
        for (t, _) in &sec {
            if !union.contains(t) {
                union.push(*t);
            }
        }
        last = Some(sec);
    }
    let ready = last
        .as_ref()
        .is_some_and(|sec| pmt_has_av(sec, min_audio));
    (ready, union, best_audio)
}

/// Go/FFmpeg `OutputURL` puts `latency` in **microseconds**; GStreamer/libsrt URI
/// expects **milliseconds**. Values > 8000 are treated as µs (same heuristic as Go).
fn normalize_srt_uri_for_gst(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(i) = rest.find("latency=") {
        out.push_str(&rest[..i]);
        out.push_str("latency=");
        rest = &rest[i + "latency=".len()..];
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let num = &rest[..end];
        if let Ok(v) = num.parse::<u64>() {
            let ms = if v > 8000 { (v / 1000).max(1) } else { v };
            out.push_str(&ms.to_string());
        } else {
            out.push_str(num);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod srt_uri_tests {
    use super::{normalize_srt_uri_for_gst, pmt_has_av};

    #[test]
    fn pmt_requires_both_video_and_aac() {
        assert!(!pmt_has_av(&[(0x0f, 0x101)], 1));
        assert!(!pmt_has_av(&[(0x1b, 0x100)], 1));
        assert!(pmt_has_av(&[(0x1b, 0x100), (0x0f, 0x101)], 1));
        assert!(pmt_has_av(&[(0x24, 0x100), (0x0f, 0x101)], 1));
        assert!(!pmt_has_av(&[(0x1b, 0x100), (0x0f, 0x101)], 4));
        assert!(pmt_has_av(
            &[
                (0x1b, 0x100),
                (0x0f, 0x101),
                (0x0f, 0x102),
                (0x0f, 0x103),
                (0x0f, 0x104),
            ],
            4
        ));
    }

    #[test]
    fn converts_ffmpeg_micros_to_gst_millis() {
        let u = normalize_srt_uri_for_gst(
            "srt://10.0.0.1:8890?latency=1000000&mode=caller&pkt_size=1316",
        );
        assert!(u.contains("latency=1000"), "{u}");
        assert!(u.contains("mode=caller"), "{u}");
    }

    #[test]
    fn leaves_millis_alone() {
        let u = normalize_srt_uri_for_gst("srt://0.0.0.0:9101?mode=listener&latency=120");
        assert!(u.contains("latency=120"), "{u}");
    }
}

pub struct ChannelPipeline {
    pub id: u32,
    pub name: String,
    /// Config key (`hq`, `proxy`, …) — what UI/Go send on preset change.
    pub encode_preset_id: String,
    pub encode_preset_label: String,
    pub status: ChannelStatus,
    pub recording: bool,
    pub srt: bool,
    pub recording_path: Option<String>,
    pub srt_url: Option<String>,
    pub last_error: Option<String>,
    device: String,
    /// User config: `auto` or a concrete DeckLink mode.
    configured_mode: String,
    /// Mode locked into the running graph.
    locked_mode: String,
    detected: Option<InputFormat>,
    preset: EncodePreset,
    udp_egress: Option<String>,
    parse_element: String,
    pipeline: Option<gstreamer::Pipeline>,
    rec_branch: Option<Branch>,
    /// Avoid relaunch storms: last adapt attempt.
    last_adapt: Option<std::time::Instant>,
    /// Peak dBFS per discrete channel (8). Updated from `level` bus messages.
    audio_peaks: [f64; 8],
    /// Live encoded bitstream rate (tee `e` sink probe).
    encode_bitrate: BitrateMeter,
    /// Live SRT MPEG-TS rate (appsrc → srtsink), only while SRT is enabled.
    srt_bitrate: Option<BitrateMeter>,
    /// Incremented on each successful `launch_locked` (SRT/preset/adapt relaunch).
    preview_epoch: u64,
}

struct Branch {
    tee_pad: gstreamer::Pad,
    /// Pads from audio tee `a` (one per AAC stereo pair).
    audio_tee_pads: Vec<gstreamer::Pad>,
    elements: Vec<gstreamer::Element>,
}

fn parse_element_for_codec(video_codec: &str) -> &'static str {
    let c = video_codec.to_ascii_lowercase();
    if roc_config::is_mezz_codec(&c) {
        // Mezz REC does not use the encoded tee parse path.
        "identity"
    } else if c.contains("265") || c.contains("hevc") {
        "h265parse"
    } else {
        "h264parse"
    }
}

impl ChannelPipeline {
    pub fn new(ch: &ChannelConfig, preset: &EncodePreset) -> Result<Self> {
        let preset_id = ch
            .encode_preset
            .clone()
            .unwrap_or_else(|| "hq".into());
        Ok(Self {
            id: ch.id,
            name: ch.name.clone(),
            encode_preset_id: preset_id,
            encode_preset_label: preset.label.clone(),
            status: ChannelStatus::Stopped,
            recording: false,
            srt: false,
            recording_path: None,
            srt_url: ch.srt_url.clone(),
            last_error: None,
            device: ch.device.clone(),
            configured_mode: ch
                .mode
                .clone()
                .unwrap_or_else(|| "auto".into()),
            locked_mode: String::new(),
            detected: None,
            parse_element: parse_element_for_codec(&preset.video_codec).to_string(),
            preset: preset.clone(),
            udp_egress: ch.udp_egress.clone(),
            pipeline: None,
            rec_branch: None,
            last_adapt: None,
            audio_peaks: [-90.0; 8],
            encode_bitrate: BitrateMeter::new(),
            srt_bitrate: None,
            preview_epoch: 0,
        })
    }

    pub fn update_config(&mut self, ch: &ChannelConfig, preset: &EncodePreset) {
        self.name = ch.name.clone();
        self.device = ch.device.clone();
        self.configured_mode = ch.mode.clone().unwrap_or_else(|| "auto".into());
        self.preset = preset.clone();
        if let Some(id) = &ch.encode_preset {
            self.encode_preset_id = id.clone();
        }
        self.encode_preset_label = preset.label.clone();
        self.parse_element = parse_element_for_codec(&preset.video_codec).to_string();
        self.udp_egress = ch.udp_egress.clone();
        if self.srt_url.is_none() {
            self.srt_url = ch.srt_url.clone();
        }
    }

    /// Apply a new encode preset. Relunches the capture graph when live (preserves REC/SRT).
    pub fn apply_encode_preset(&mut self, preset_id: &str, preset: &EncodePreset) -> Result<()> {
        if self.recording {
            bail!("stop recording before changing encode preset");
        }
        let live = matches!(
            self.status,
            ChannelStatus::Running | ChannelStatus::Waiting
        );
        self.encode_preset_id = preset_id.to_string();
        self.encode_preset_label = preset.label.clone();
        self.preset = preset.clone();
        self.parse_element = parse_element_for_codec(&preset.video_codec).to_string();
        if !live {
            tracing::info!(
                channel = self.id,
                preset_id,
                "encode preset updated (applies on next start)"
            );
            return Ok(());
        }
        let mode = if self.locked_mode.is_empty() {
            self.configured_mode.clone()
        } else {
            self.locked_mode.clone()
        };
        tracing::info!(
            channel = self.id,
            preset_id,
            %mode,
            "encode preset changed — relaunching capture"
        );
        self.relaunch_preserving_branches(&mode)
    }

    fn relaunch_preserving_branches(&mut self, mode: &str) -> Result<()> {
        let was_rec = self.recording;
        let rec_path = self.recording_path.clone();
        // Keep self.srt / self.srt_url — launch_locked bakes SRT into the graph.

        let _ = self.detach_recording(false);
        if let Some(p) = self.pipeline.take() {
            let _ = p.set_state(gstreamer::State::Null);
        }
        self.recording = false;
        self.recording_path = None;
        self.srt_bitrate = None;
        if mode != "auto" && !mode.is_empty() {
            self.locked_mode = mode.to_string();
        }
        let launch_mode = if self.locked_mode.is_empty() {
            "auto".to_string()
        } else {
            self.locked_mode.clone()
        };
        match self.launch_locked(&launch_mode) {
            Ok(()) => {
                if was_rec {
                    if let Some(path) = rec_path {
                        if let Err(err) = self.start_recording(&path) {
                            tracing::error!(
                                channel = self.id,
                                error = %err,
                                "failed to re-attach recording after relaunch"
                            );
                        }
                    }
                }
                Ok(())
            }
            Err(err) => {
                self.status = ChannelStatus::Error;
                self.last_error = Some(err.to_string());
                Err(err)
            }
        }
    }

    pub fn start(&mut self) -> Result<()> {
        if self.pipeline.is_some() {
            return Ok(());
        }
        let locked = self.resolve_lock_mode()?;
        self.locked_mode = locked.clone();
        self.launch_locked(&locked)?;
        Ok(())
    }

    fn resolve_lock_mode(&mut self) -> Result<String> {
        if !is_auto_mode(&self.configured_mode) {
            return Ok(self.configured_mode.clone());
        }
        match probe_input_format(&self.device, 3500) {
            Ok(fmt) => {
                tracing::info!(
                    channel = self.id,
                    format = %fmt.summary(),
                    "probed input format"
                );
                self.detected = Some(fmt.clone());
                Ok(fmt.mode)
            }
            Err(err) => {
                tracing::warn!(
                    channel = self.id,
                    error = %err,
                    "input probe failed — falling back to 1080p50"
                );
                Ok("1080p50".into())
            }
        }
    }

    fn launch_locked(&mut self, locked: &str) -> Result<()> {
        let hls_base = std::env::var("ROC_MEDIA_HLS_DIR").unwrap_or_else(|_| {
            "/opt/applications/roc-media-engine/hls".into()
        });
        let hls_dir = format!("{hls_base}/{}", self.id);
        let _ = std::fs::create_dir_all(&hls_dir);
        // Drop stale preview playlists/segments so players don't stick on old gens.
        if let Ok(rd) = std::fs::read_dir(&hls_dir) {
            for ent in rd.flatten() {
                let name = ent.file_name();
                let n = name.to_string_lossy();
                if n == "preview.m3u8"
                    || n.ends_with(".ts")
                    || n.starts_with("listen_")
                    || n.starts_with("pv")
                    || n == "thumb.jpg"
                {
                    let _ = std::fs::remove_file(ent.path());
                }
            }
        }
        let playlist = format!("{hls_dir}/preview.m3u8");
        // Graph always includes `appsink srt_in` when UDP is configured. SRT
        // publish is armed separately so REC never needs a relaunch.
        let launch = build_capture_encode_once_launch(&CaptureLaunchOpts {
            device: self.device.clone(),
            mode: locked.to_string(),
            preset: self.preset.clone(),
            preview_path: Some(playlist),
            record_path: None,
            srt_url: None,
            udp_egress: self.udp_egress.clone(),
            with_tee_preview: true,
        });
        tracing::info!(channel = self.id, mode = %locked, %launch, "capture pipeline");
        let pipeline = gstreamer::parse::launch(&launch)
            .with_context(|| format!("parse capture launch ch{}", self.id))?
            .downcast::<gstreamer::Pipeline>()
            .map_err(|_| anyhow!("capture launch did not yield Pipeline"))?;

        // Re-arm SRT after preset/adapt relaunch if it was already publishing.
        self.srt_bitrate = None;
        if self.srt {
            if let Some(uri) = self.srt_url.as_deref().map(normalize_srt_uri_for_gst) {
                if let Err(err) = arm_srt_appsink_gate(self.id, &pipeline, &uri, 1) {
                    let _ = pipeline.set_state(gstreamer::State::Null);
                    return Err(err).context(format!("SRT appsink gate ch{}", self.id));
                }
                let meter = BitrateMeter::new();
                if let Some(src) = pipeline.by_name(&format!("srt_out_{}", self.id)) {
                    if let Some(pad) = src.static_pad("src") {
                        let _ = meter.attach_probe(&pad);
                    }
                }
                self.srt_bitrate = Some(meter);
            }
        }

        if let Err(err) = pipeline.set_state(gstreamer::State::Playing) {
            let _ = pipeline.set_state(gstreamer::State::Null);
            return Err(err).context("capture set PLAYING");
        }
        // Live encode bitrate: count buffers into encoded tee `e`.
        self.encode_bitrate.reset();
        if let Some(tee) = pipeline.by_name("e") {
            if let Some(sink) = tee.static_pad("sink") {
                let _ = self.encode_bitrate.attach_probe(&sink);
            }
        }
        self.preview_epoch = self.preview_epoch.wrapping_add(1);
        self.pipeline = Some(pipeline);
        self.status = ChannelStatus::Waiting;
        self.last_error = None;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        let _ = self.detach_recording(false);
        if let Some(p) = self.pipeline.take() {
            let _ = p.send_event(gstreamer::event::Eos::new());
            let _ = p.set_state(gstreamer::State::Null);
        }
        self.status = ChannelStatus::Stopped;
        self.recording = false;
        self.srt = false;
        self.recording_path = None;
        self.audio_peaks = [-90.0; 8];
        self.encode_bitrate.reset();
        self.srt_bitrate = None;
        Ok(())
    }

    pub fn start_recording(&mut self, path: &str) -> Result<()> {
        if self.pipeline.is_none() {
            bail!("capture not running");
        }
        if self.recording {
            bail!("already recording");
        }
        self.attach_recording(path)?;
        self.recording = true;
        self.recording_path = Some(path.to_string());
        Ok(())
    }

    pub fn stop_recording(&mut self) -> Result<()> {
        if !self.recording {
            return Ok(());
        }
        self.detach_recording(true)?;
        self.recording = false;
        self.recording_path = None;
        Ok(())
    }

    pub fn start_srt(&mut self, url: &str) -> Result<()> {
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow!("capture not running"))?
            .clone();
        if pipeline.by_name("srt_in").is_none() {
            bail!("SRT appsink missing — channel has no MPEG-TS egress");
        }
        let gst_url = normalize_srt_uri_for_gst(url);
        self.srt_url = Some(url.to_string());
        if self.srt {
            if let Some(sink) = pipeline.by_name(&format!("srt_sink_{}", self.id)) {
                sink.set_property("uri", &gst_url);
                tracing::info!(channel = self.id, %gst_url, "updated SRT sink URI");
            }
            return Ok(());
        }
        // Hot-attach publish path only — REC / UDP / preview keep running.
        disarm_srt_appsink_gate(self.id, &pipeline);
        arm_srt_appsink_gate(self.id, &pipeline, &gst_url, 1)
            .context("SRT appsink gate")?;
        let meter = BitrateMeter::new();
        if let Some(src) = pipeline.by_name(&format!("srt_out_{}", self.id)) {
            if let Some(pad) = src.static_pad("src") {
                let _ = meter.attach_probe(&pad);
            }
        }
        self.srt_bitrate = Some(meter);
        self.srt = true;
        tracing::info!(channel = self.id, %url, gst_uri = %gst_url, "SRT publish attached (no relaunch)");
        Ok(())
    }

    pub fn stop_srt(&mut self) -> Result<()> {
        if !self.srt {
            return Ok(());
        }
        if let Some(pipeline) = self.pipeline.as_ref() {
            disarm_srt_appsink_gate(self.id, pipeline);
        }
        self.srt = false;
        self.srt_bitrate = None;
        tracing::info!(channel = self.id, "SRT publish detached (REC undisturbed)");
        Ok(())
    }

    fn encoded_tee(&self) -> Result<gstreamer::Element> {
        let p = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow!("no pipeline"))?;
        p.by_name("e")
            .ok_or_else(|| anyhow!("encoded tee `e` missing — is capture running?"))
    }

    fn raw_tee(&self) -> Result<gstreamer::Element> {
        let p = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow!("no pipeline"))?;
        p.by_name("t")
            .ok_or_else(|| anyhow!("raw tee `t` missing — is capture running?"))
    }

    fn attach_recording(&mut self, path: &str) -> Result<()> {
        if roc_config::is_mezz_codec(&self.preset.video_codec) {
            self.attach_mezz_recording(path)
        } else {
            self.attach_encoded_recording(path)
        }
    }

    /// NVENC bitstream REC from encoded tee `e` → mp4mux.
    fn attach_encoded_recording(&mut self, path: &str) -> Result<()> {
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow!("no pipeline"))?
            .clone();
        let tee = self.encoded_tee()?;
        let audio_tee = pipeline.by_name("a");

        let queue_v = gstreamer::ElementFactory::make("queue")
            .name(format!("q_rec_v_{}", self.id))
            .build()
            .context("queue video")?;
        let parse = gstreamer::ElementFactory::make(&self.parse_element)
            .name(format!("parse_rec_{}", self.id))
            .build()
            .with_context(|| format!("make {}", self.parse_element))?;
        // Keep SPS/PPS (or VPS/SPS/PPS for HEVC) in-band for mid-stream joiners.
        let _ = parse.set_property_from_str("config-interval", "-1");
        // Progressive MP4 (moov at end). Fragmented/streamable files often look
        // "corrupt" in browsers and desktop players when finalize is incomplete.
        let mux = gstreamer::ElementFactory::make("mp4mux")
            .name(format!("mux_rec_{}", self.id))
            .property("fragment-duration", 0u32)
            .property("streamable", false)
            .build()
            .context("mp4mux")?;
        let sink = gstreamer::ElementFactory::make("filesink")
            .name(format!("fs_rec_{}", self.id))
            .property("location", path)
            .property("sync", false)
            // Ensure CIFS/NFS sees bytes promptly; helps avoid 0-byte stubs on stop.
            .property("async", false)
            .build()
            .context("filesink")?;

        let id_v = make_mux_ts_align(&format!("id_rec_v_{}", self.id))?;
        let mut elements = vec![
            queue_v.clone(),
            parse.clone(),
            id_v.clone(),
            mux.clone(),
            sink.clone(),
        ];
        pipeline.add_many([&queue_v, &parse, &id_v, &mux, &sink])?;
        queue_v.link(&parse).context("link rec queue→parse")?;
        parse.link(&id_v).context("link rec parse→identity")?;
        id_v.link(&mux).context("link rec identity→mux")?;
        mux.link(&sink).context("link rec mux→sink")?;

        // Attach AAC now so mp4mux registers the audio track, but gate buffers
        // until the first video keyframe. Otherwise AAC starts immediately while
        // video waits up to one GOP (~1s) for IDR — both get pts=0 via
        // single-segment → permanent ~1s lipsync error.
        let av_gate = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut audio_tee_pads = Vec::new();
        if let Some(a_tee) = audio_tee {
            match self.link_program_aac(&pipeline, &a_tee, &mux, "rec", true) {
                Ok((a_pads, audio_els)) => {
                    // Gate every AAC queue until the first video keyframe.
                    for el in &audio_els {
                        if el.name().starts_with("q_rec_a") {
                            install_av_start_gate(el, av_gate.clone());
                        }
                    }
                    audio_tee_pads = a_pads;
                    elements.extend(audio_els);
                    install_mux_av_sync_log("rec");
                }
                Err(err) => {
                    tracing::warn!(
                        channel = self.id,
                        error = %err,
                        "REC video-only — program AAC attach failed"
                    );
                }
            }
        } else {
            tracing::warn!(
                channel = self.id,
                "REC video-only — audio tee `a` missing"
            );
        }

        for el in &elements {
            el.sync_state_with_parent()
                .context("sync_state_with_parent record")?;
        }

        // Open the audio gate on the first video keyframe (non-DELTA_UNIT).
        if !audio_tee_pads.is_empty() {
            arm_av_gate_on_keyframe(&id_v, av_gate);
        }

        let tee_pad = tee
            .request_pad_simple("src_%u")
            .ok_or_else(|| anyhow!("tee request_pad failed"))?;
        let sink_pad = queue_v
            .static_pad("sink")
            .ok_or_else(|| anyhow!("queue sink pad"))?;
        tee_pad
            .link(&sink_pad)
            .context("link tee → record queue")?;

        tracing::info!(
            channel = self.id,
            %path,
            audio_pairs = audio_tee_pads.len(),
            "attached record branch (no relaunch)"
        );
        self.rec_branch = Some(Branch {
            tee_pad,
            audio_tee_pads,
            elements,
        });
        Ok(())
    }

    /// Mezz REC from raw tee `t`: DNxHD (.mov) or XAVC Intra (.mxf) + NTP/RTC timecode.
    fn attach_mezz_recording(&mut self, path: &str) -> Result<()> {
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow!("no pipeline"))?
            .clone();
        let tee = self.raw_tee()?;
        let audio_tee = pipeline.by_name("a");
        let codec = self.preset.video_codec.to_ascii_lowercase();
        let bitrate = crate::parse_bitrate(&self.preset.video_bitrate).unwrap_or(185_000_000);

        let queue_v = gstreamer::ElementFactory::make("queue")
            .name(format!("q_rec_v_{}", self.id))
            .property("max-size-buffers", 8u32)
            .property("max-size-bytes", 0u32)
            .property("max-size-time", gstreamer::ClockTime::from_seconds(1))
            .build()
            .context("mezz queue")?;
        let convert = gstreamer::ElementFactory::make("videoconvert")
            .name(format!("vconv_rec_{}", self.id))
            .build()
            .context("videoconvert")?;
        let caps = gstreamer::ElementFactory::make("capsfilter")
            .name(format!("caps_rec_{}", self.id))
            .property(
                "caps",
                gstreamer::Caps::from_str("video/x-raw,format=Y42B")
                    .context("Y42B caps")?,
            )
            .build()
            .context("capsfilter")?;
        // RTC source uses the host clock (chrony/NTP). No PTP on this machine.
        let tc = gstreamer::ElementFactory::make("timecodestamper")
            .name(format!("tc_rec_{}", self.id))
            .build()
            .context("timecodestamper")?;
        let _ = tc.set_property_from_str("source", "rtc");
        let _ = tc.set_property_from_str("set", "always");

        let (enc, parse_opt, mux_name): (gstreamer::Element, Option<gstreamer::Element>, &str) =
            if codec.contains("dnx") {
                let enc = gstreamer::ElementFactory::make("avenc_dnxhd")
                    .name(format!("enc_rec_{}", self.id))
                    .property("bitrate", bitrate as i32)
                    .build()
                    .context("avenc_dnxhd")?;
                let _ = enc.set_property_from_str("profile", "dnxhd");
                (enc, None, "qtmux")
            } else if codec.contains("xavc") {
                let enc = gstreamer::ElementFactory::make("x264enc")
                    .name(format!("enc_rec_{}", self.id))
                    .property("bitrate", (bitrate / 1000) as u32)
                    .property("key-int-max", 1u32)
                    .property("bframes", 0u32)
                    .build()
                    .context("x264enc xavc-intra")?;
                let _ = enc.set_property_from_str("speed-preset", "medium");
                let _ = enc.set_property_from_str("tune", "zerolatency");
                let parse = gstreamer::ElementFactory::make("h264parse")
                    .name(format!("parse_rec_{}", self.id))
                    .build()
                    .context("h264parse")?;
                let _ = parse.set_property_from_str("config-interval", "-1");
                (enc, Some(parse), "mxfmux")
            } else {
                bail!("unsupported mezz codec {}", self.preset.video_codec);
            };

        let mut mux_builder = gstreamer::ElementFactory::make(mux_name)
            .name(format!("mux_rec_{}", self.id));
        if mux_name == "qtmux" {
            mux_builder = mux_builder.property("force-create-timecode-trak", true);
        }
        let mux = mux_builder.build().with_context(|| format!("make {mux_name}"))?;
        let sink = gstreamer::ElementFactory::make("filesink")
            .name(format!("fs_rec_{}", self.id))
            .property("location", path)
            .property("sync", false)
            .property("async", false)
            .build()
            .context("filesink")?;

        let mut elements = vec![
            queue_v.clone(),
            convert.clone(),
            caps.clone(),
            tc.clone(),
            enc.clone(),
            mux.clone(),
            sink.clone(),
        ];
        pipeline.add_many([
            &queue_v, &convert, &caps, &tc, &enc, &mux, &sink,
        ])?;
        queue_v.link(&convert).context("mezz queue→convert")?;
        convert.link(&caps).context("mezz convert→caps")?;
        caps.link(&tc).context("mezz caps→timecode")?;
        tc.link(&enc).context("mezz timecode→enc")?;
        if let Some(parse) = &parse_opt {
            pipeline.add(parse)?;
            // High 4:2:2 Intra profile for XAVC-HD-style Intra.
            let profile_caps = gstreamer::ElementFactory::make("capsfilter")
                .name(format!("prof_rec_{}", self.id))
                .property(
                    "caps",
                    gstreamer::Caps::from_str("video/x-h264,profile=high-4:2:2-intra")
                        .context("xavc profile caps")?,
                )
                .build()
                .context("profile capsfilter")?;
            pipeline.add(&profile_caps)?;
            enc.link(&profile_caps).context("xavc enc→profile")?;
            profile_caps.link(parse).context("xavc profile→parse")?;
            parse.link(&mux).context("xavc parse→mux")?;
            elements.push(profile_caps);
            elements.push(parse.clone());
        } else {
            enc.link(&mux).context("dnxhd enc→mux")?;
        }
        mux.link(&sink).context("mezz mux→sink")?;

        let mut audio_tee_pads = Vec::new();
        if let Some(a_tee) = audio_tee {
            let audio_res = if mux_name == "mxfmux" {
                self.link_mezz_pcm(&pipeline, &a_tee, &mux, "rec")
            } else {
                self.link_program_aac(&pipeline, &a_tee, &mux, "rec", true)
            };
            match audio_res {
                Ok((a_pads, audio_els)) => {
                    audio_tee_pads = a_pads;
                    elements.extend(audio_els);
                }
                Err(err) => {
                    tracing::warn!(
                        channel = self.id,
                        error = %err,
                        "mezz REC video-only — audio attach failed"
                    );
                }
            }
        }

        for el in &elements {
            el.sync_state_with_parent()
                .context("sync_state_with_parent mezz record")?;
        }

        let tee_pad = tee
            .request_pad_simple("src_%u")
            .ok_or_else(|| anyhow!("raw tee request_pad failed"))?;
        let sink_pad = queue_v
            .static_pad("sink")
            .ok_or_else(|| anyhow!("queue sink pad"))?;
        tee_pad
            .link(&sink_pad)
            .context("link raw tee → mezz record")?;

        tracing::info!(
            channel = self.id,
            %path,
            codec = %self.preset.video_codec,
            mux = mux_name,
            bitrate,
            audio_pads = audio_tee_pads.len(),
            "attached mezz record branch (NTP/RTC timecode)"
        );
        self.rec_branch = Some(Branch {
            tee_pad,
            audio_tee_pads,
            elements,
        });
        Ok(())
    }

    /// Stereo PCM into MXF (XAVC path) — first pair only.
    fn link_mezz_pcm(
        &self,
        pipeline: &gstreamer::Pipeline,
        a_tee: &gstreamer::Element,
        mux: &gstreamer::Element,
        tag: &str,
    ) -> Result<(Vec<gstreamer::Pad>, Vec<gstreamer::Element>)> {
        let matrix = stereo_pair_matrix(0);
        let queue_a = gstreamer::ElementFactory::make("queue")
            .name(format!("q_{tag}_pcm_{}", self.id))
            .property("max-size-buffers", 64u32)
            .property("max-size-bytes", 0u32)
            .property("max-size-time", gstreamer::ClockTime::from_mseconds(250))
            .build()
            .context("pcm queue")?;
        let desc = format!(
            "audioconvert mix-matrix=\"{matrix}\" ! \
             audio/x-raw,format=S24LE,channels=2,rate=48000"
        );
        let bin = gstreamer::parse::bin_from_description(&desc, true)
            .context("mezz pcm bin")?;
        let bin_el: gstreamer::Element = bin.upcast();
        bin_el.set_property("name", format!("pcm_{tag}_{}", self.id));
        pipeline.add_many([&queue_a, &bin_el])?;
        queue_a
            .link(&bin_el)
            .context("link pcm queue→bin")?;
        bin_el.link(mux).context("link pcm→mxfmux")?;

        let tee_pad = a_tee
            .request_pad_simple("src_%u")
            .ok_or_else(|| anyhow!("audio tee pad"))?;
        let sink = queue_a
            .static_pad("sink")
            .ok_or_else(|| anyhow!("pcm queue sink"))?;
        tee_pad.link(&sink).context("link audio tee→pcm")?;
        Ok((vec![tee_pad], vec![queue_a, bin_el]))
    }

    /// Stereo AAC pair(s) into an existing mux (mp4mux / mpegtsmux).
    ///
    /// `audio_channels >= 8` → four AAC stereo pairs (same as Go/FFmpeg).
    /// `ts_align`: insert `identity single-segment` (needed for REC mp4mux late-join).
    /// Leave false for live MPEG-TS/SRT — gating/restamp there makes PMT video-only.
    fn link_program_aac(
        &self,
        pipeline: &gstreamer::Pipeline,
        a_tee: &gstreamer::Element,
        mux: &gstreamer::Element,
        tag: &str,
        ts_align: bool,
    ) -> Result<(Vec<gstreamer::Pad>, Vec<gstreamer::Element>)> {
        let pairs = aac_stereo_pairs(self.preset.audio_channels);
        let hold_ms = if ts_align { 40u64 } else { 0 };
        let aac_bps = crate::parse_bitrate(&self.preset.audio_bitrate).unwrap_or(192_000);
        let mut pads = Vec::with_capacity(pairs);
        let mut els = Vec::new();

        for pair in 0..pairs {
            let queue_a = gstreamer::ElementFactory::make("queue")
                .name(format!("q_{tag}_a{pair}_{}", self.id))
                .property(
                    "min-threshold-time",
                    gstreamer::ClockTime::from_mseconds(hold_ms),
                )
                .property("max-size-buffers", 64u32)
                .property("max-size-bytes", 0u32)
                .property("max-size-time", gstreamer::ClockTime::from_mseconds(250))
                .build()
                .context("queue audio")?;
            // MediaMTX (and MPEG-TS stream type 0x0F) expects ADTS-framed AAC, not raw.
            let matrix = stereo_pair_matrix(pair);
            let desc = format!(
                "audioconvert mix-matrix=\"{matrix}\" ! \
                 audio/x-raw,channels=2 ! voaacenc bitrate={aac_bps} ! aacparse ! \
                 capsfilter caps=audio/mpeg,mpegversion=4,stream-format=adts"
            );
            let abin = gstreamer::parse::bin_from_description(&desc, true)
                .with_context(|| format!("parse {tag} audio bin pair {pair}"))?;
            abin.set_property("name", format!("{tag}_a{pair}_bin_{}", self.id));
            let abin_el: gstreamer::Element = abin.upcast();

            pipeline
                .add_many([&queue_a, &abin_el])
                .context("add program audio elements")?;
            queue_a
                .link(&abin_el)
                .context("link audio queue → bin")?;
            els.push(queue_a.clone());
            els.push(abin_el.clone());

            if ts_align {
                let id_a = make_mux_ts_align(&format!("id_{tag}_a{pair}_{}", self.id))?;
                pipeline.add(&id_a).context("add audio identity")?;
                abin_el
                    .link(&id_a)
                    .context("link audio bin → identity")?;
                id_a
                    .link(mux)
                    .context("link audio identity → mux")?;
                els.push(id_a);
            } else {
                abin_el
                    .link(mux)
                    .context("link audio bin → mux")?;
            }

            let a_pad = a_tee
                .request_pad_simple("src_%u")
                .ok_or_else(|| anyhow!("audio tee request_pad failed"))?;
            let a_sink = queue_a
                .static_pad("sink")
                .ok_or_else(|| anyhow!("audio queue sink"))?;
            a_pad
                .link(&a_sink)
                .context("link audio tee → queue")?;
            pads.push(a_pad);
        }
        Ok((pads, els))
    }

    fn detach_recording(&mut self, finalize: bool) -> Result<()> {
        let Some(branch) = self.rec_branch.take() else {
            return Ok(());
        };
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow!("no pipeline"))?
            .clone();
        let tee = self.encoded_tee()?;

        // 1) Cut data path from tee before touching downstream state.
        Self::unlink_branch(&pipeline, &tee, &branch);

        // 2) Optional EOS for a cleaner mp4 footer. Do NOT wait on the pipeline bus:
        //    that raced under multi-channel stop and held the global lock for seconds.
        if finalize {
            use gstreamer::{PadProbeReturn, PadProbeType};
            use std::sync::mpsc;

            let (tx, rx) = mpsc::channel::<()>();
            // Keep probe id alive until after recv — dropping it removes the probe.
            let eos_probe = branch
                .elements
                .iter()
                .find(|e| e.name().starts_with("fs_rec"))
                .and_then(|sink| sink.static_pad("sink"))
                .map(|pad| {
                    let tx = tx.clone();
                    pad.add_probe(PadProbeType::EVENT_DOWNSTREAM, move |_, info| {
                        if let Some(ev) = info.event() {
                            if ev.type_() == gstreamer::EventType::Eos {
                                let _ = tx.send(());
                                return PadProbeReturn::Remove;
                            }
                        }
                        PadProbeReturn::Ok
                    })
                });
            drop(tx);

            // Inject EOS *into* each record queue sink (downstream). Element-level
            // send_event(EOS) on a filter goes to its sink pads (upstream) and
            // never reaches mp4mux — leaving a moov-less "corrupt" file.
            for el in &branch.elements {
                let name = el.name();
                if name.starts_with("q_rec") {
                    if let Some(sink) = el.static_pad("sink") {
                        let _ = sink.send_event(gstreamer::event::Eos::new());
                    }
                }
            }
            // Progressive mp4mux writes moov only on EOS — wait generously.
            match rx.recv_timeout(std::time::Duration::from_millis(3000)) {
                Ok(()) => tracing::info!(channel = self.id, "record EOS reached filesink"),
                Err(_) => tracing::warn!(
                    channel = self.id,
                    "record EOS timeout — moov may be missing"
                ),
            }
            drop(eos_probe);
        }

        // 3) Null from sink → source, then remove.
        for el in branch.elements.iter().rev() {
            let _ = el.set_state(gstreamer::State::Null);
        }
        for el in &branch.elements {
            let _ = pipeline.remove(el);
        }
        tracing::info!(channel = self.id, "detached record branch");
        Ok(())
    }

    fn unlink_branch(
        pipeline: &gstreamer::Pipeline,
        video_tee: &gstreamer::Element,
        branch: &Branch,
    ) {
        if let Some(qpad) = branch.elements.first().and_then(|e| e.static_pad("sink")) {
            let _ = branch.tee_pad.unlink(&qpad);
        }
        video_tee.release_request_pad(&branch.tee_pad);
        if let Some(a_tee) = pipeline.by_name("a") {
            for audio_pad in &branch.audio_tee_pads {
                if let Some(peer) = audio_pad.peer() {
                    let _ = audio_pad.unlink(&peer);
                }
                a_tee.release_request_pad(audio_pad);
            }
        }
    }

    pub fn poll_bus(&mut self) {
        {
            let Some(p) = self.pipeline.as_ref() else {
                return;
            };
            let (_, cur, _) = p.state(gstreamer::ClockTime::ZERO);
            if cur == gstreamer::State::Playing && self.status != ChannelStatus::Error {
                self.status = ChannelStatus::Running;
            }
        }

        let adapt_to = {
            if let Some(fmt) = self.read_live_format() {
                let changed = self
                    .detected
                    .as_ref()
                    .map(|d| d.mode != fmt.mode)
                    .unwrap_or(true);
                self.detected = Some(fmt.clone());
                let should = is_auto_mode(&self.configured_mode)
                    && fmt.mode != self.locked_mode
                    && fmt.width >= 1280
                    && self
                        .last_adapt
                        .map(|t| t.elapsed() > std::time::Duration::from_secs(3))
                        .unwrap_or(true);
                if should {
                    Some(fmt.mode)
                } else {
                    if changed {
                        tracing::info!(
                            channel = self.id,
                            format = %fmt.summary(),
                            "input format"
                        );
                    }
                    None
                }
            } else {
                None
            }
        };
        if let Some(mode) = adapt_to {
            tracing::warn!(
                channel = self.id,
                from = %self.locked_mode,
                to = %mode,
                "input format changed — adapting capture"
            );
            let _ = self.adapt_to_mode(&mode);
        }

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
                MessageView::Eos(_) => {}
                MessageView::StateChanged(sc) => {
                    if sc
                        .src()
                        .map(|s| s == p.upcast_ref::<gstreamer::Object>())
                        .unwrap_or(false)
                        && sc.current() == gstreamer::State::Playing
                    {
                        self.status = ChannelStatus::Running;
                    }
                }
                MessageView::Warning(w) => {
                    let text = format!("{} ({})", w.error(), w.debug().unwrap_or_default());
                    tracing::warn!(channel = self.id, %text, "gst warning");
                }
                MessageView::Element(el) => {
                    if let Some(s) = el.structure() {
                        if s.name() == "level" {
                            Self::apply_level_structure(&mut self.audio_peaks, s);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn apply_level_structure(peaks: &mut [f64; 8], s: &gstreamer::StructureRef) {
        // `peak` is a GValueArray of dBFS doubles (already converted by `level`).
        let Ok(arr) = s.get::<gstreamer::glib::ValueArray>("peak") else {
            return;
        };
        for (i, val) in arr.iter().enumerate().take(8) {
            let Ok(db) = val.get::<f64>() else {
                continue;
            };
            peaks[i] = if db.is_finite() { db.max(-90.0) } else { -90.0 };
        }
    }

    fn read_live_format(&self) -> Option<InputFormat> {
        let p = self.pipeline.as_ref()?;
        let dl = p.by_name("dlsrc")?;
        let pad = dl.static_pad("src")?;
        let caps = pad.current_caps()?;
        format_from_caps(&caps)
    }

    fn adapt_to_mode(&mut self, new_mode: &str) -> Result<()> {
        self.last_adapt = Some(std::time::Instant::now());
        self.locked_mode = new_mode.to_string();
        self.relaunch_preserving_branches(new_mode)
    }

    pub fn snapshot(&self, nvenc_slots_used: usize) -> ChannelSnapshot {
        ChannelSnapshot {
            id: self.id,
            name: self.name.clone(),
            status: self.status,
            encode_preset: self.encode_preset_id.clone(),
            video_bitrate_kbps: self.encode_bitrate.kbps(),
            srt_bitrate_kbps: self.srt_bitrate.as_ref().and_then(|m| m.kbps()),
            recording: self.recording,
            srt: self.srt,
            recording_path: self.recording_path.clone(),
            srt_url: self.srt_url.clone(),
            last_error: self.last_error.clone(),
            nvenc_slots_used,
            configured_mode: self.configured_mode.clone(),
            locked_mode: if self.locked_mode.is_empty() {
                None
            } else {
                Some(self.locked_mode.clone())
            },
            input_format: self.detected.as_ref().map(|f| f.summary()),
            audio_peaks: Some(self.audio_peaks.to_vec()),
            preview_epoch: self.preview_epoch,
        }
    }
}
