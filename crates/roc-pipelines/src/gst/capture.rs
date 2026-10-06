//! Per-channel GStreamer capture graph.
//!
//! Two independent REC roles attach as dynamic branches and can run at the same
//! time (each with its own start/stop):
//! - **proxy**: encoded tee `e` → encode-preset parser → mp4mux (`.mp4`);
//! - **hq**: record preset — mezz from tee `raw` (DNxHD → `.mxf`) or `t` (ProRes → `.mov`),
//!   or the encoded bitstream from tee `e` (`.mp4`) for NVENC record presets.
//!
//! Element names carry the role tag (`q_proxy_*` / `q_hq_*`) and each branch
//! remembers its video tee so detach releases the right pad. REC is independent
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
use crate::signal_format::{format_from_caps, is_auto_mode, InputFormat};
use crate::{ChannelSnapshot, ChannelStatus, RecordingRole};

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

/// Hot-attach onto a live tee often skips a fresh TIME segment, so
/// `identity single-segment` alone leaves PTS in the running domain and
/// `mxfmux` aborts on `index_pos_diff`. Rewrite PTS/DTS from 0 with a fixed
/// frame duration derived from caps (fallback 50 fps).
///
/// When `video_pts_ns` is set (ProRes/`qtmux`), publish the rewritten PTS so
/// PCM can be paced and not run ahead of a slow software encode.
fn arm_mezz_pts_reset(
    video_identity: &gstreamer::Element,
    frame_duration_ns: u64,
    video_pts_ns: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
) {
    use gstreamer::{ClockTime, PadProbeReturn, PadProbeType};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    let Some(pad) = video_identity.static_pad("src") else {
        return;
    };
    let base = Arc::new(AtomicU64::new(u64::MAX));
    let frame_i = Arc::new(AtomicU64::new(0));
    pad.add_probe(PadProbeType::BUFFER, move |_, info| {
        let Some(buf) = info.buffer_mut() else {
            return PadProbeReturn::Ok;
        };
        let buf = buf.make_mut();
        let pts_ns = buf.pts().map(|t| t.nseconds()).unwrap_or(0);
        let mut b = base.load(Ordering::SeqCst);
        if b == u64::MAX {
            base.store(pts_ns, Ordering::SeqCst);
            b = pts_ns;
        }
        let i = frame_i.fetch_add(1, Ordering::SeqCst);
        // Prefer sequential CFR timestamps — more stable for mxfmux than raw delta.
        let out = i.saturating_mul(frame_duration_ns);
        buf.set_pts(ClockTime::from_nseconds(out));
        buf.set_dts(ClockTime::from_nseconds(out));
        buf.set_duration(ClockTime::from_nseconds(frame_duration_ns));
        if let Some(pub_pts) = video_pts_ns.as_ref() {
            pub_pts.store(out, Ordering::SeqCst);
        }
        let _ = b; // base kept for diagnostics if we switch back to delta mode
        PadProbeReturn::Ok
    });
}

/// Rebase PCM to 0 and optionally drop buffers that run ahead of video PTS.
/// `qtmux` rejects audio that drifts ahead of a slower ProRes encode.
fn arm_mezz_audio_pts_reset(
    audio_identity: &gstreamer::Element,
    video_pts_ns: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
) {
    use gstreamer::{ClockTime, PadProbeReturn, PadProbeType};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    let Some(pad) = audio_identity.static_pad("src") else {
        return;
    };
    let base = Arc::new(AtomicU64::new(u64::MAX));
    // Allow a little lead so short PCM bursts don't starve; ~80 ms at 48 kHz.
    const MAX_LEAD_NS: u64 = 80_000_000;
    pad.add_probe(PadProbeType::BUFFER, move |_, info| {
        let Some(buf) = info.buffer_mut() else {
            return PadProbeReturn::Ok;
        };
        let buf = buf.make_mut();
        let pts_ns = buf.pts().map(|t| t.nseconds()).unwrap_or(0);
        let mut b = base.load(Ordering::SeqCst);
        if b == u64::MAX {
            base.store(pts_ns, Ordering::SeqCst);
            b = pts_ns;
        }
        let out = pts_ns.saturating_sub(b);
        if let Some(vpts) = video_pts_ns.as_ref() {
            let v = vpts.load(Ordering::SeqCst);
            // Wait until video has produced at least one rewritten frame.
            if v == u64::MAX || out > v.saturating_add(MAX_LEAD_NS) {
                return PadProbeReturn::Drop;
            }
        }
        buf.set_pts(ClockTime::from_nseconds(out));
        if let Some(dts) = buf.dts() {
            buf.set_dts(ClockTime::from_nseconds(dts.nseconds().saturating_sub(b)));
        } else {
            buf.set_dts(ClockTime::from_nseconds(out));
        }
        PadProbeReturn::Ok
    });
}

/// Wire `appsink srt_in` → gated `appsrc` → `srtsink`.
///
/// Pad-probe Drop was leaking poison BUFFER_LISTs (open-chunk dump was clean A/V
/// while the SRT capture still started with an older AAC-only PMT). Pulling in
/// userspace and only pushing approved buffers matches how we must control the
/// first bytes MediaMTX locks on. HydraSRT avoids this by never remuxing; we
/// still encode, so the gate stays.
pub(crate) fn arm_srt_appsink_gate(
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
pub(crate) fn disarm_srt_appsink_gate(channel: u32, pipeline: &gstreamer::Pipeline) {
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
pub(crate) fn normalize_srt_uri_for_gst(raw: &str) -> String {
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
    /// Live/proxy preset key (`hq`, `proxy`, …).
    pub encode_preset_id: String,
    pub encode_preset_label: String,
    /// REC preset key (may be mezz).
    pub record_preset_id: String,
    pub record_preset_label: String,
    pub status: ChannelStatus,
    /// Proxy REC (encoded tee `e`, encode preset codec → .mp4) is attached.
    pub proxy_recording: bool,
    /// HQ REC (record preset: mezz from tee `t`, else encoded from tee `e`) is attached.
    pub hq_recording: bool,
    pub srt: bool,
    pub proxy_recording_path: Option<String>,
    pub hq_recording_path: Option<String>,
    pub srt_url: Option<String>,
    pub last_error: Option<String>,
    device: String,
    /// User config: `auto` or a concrete DeckLink mode.
    configured_mode: String,
    /// Mode locked into the running graph.
    locked_mode: String,
    detected: Option<InputFormat>,
    /// Live/proxy encode settings (NVENC path).
    preset: EncodePreset,
    /// Recording encode settings (NVENC bitstream or mezz from raw tee).
    record_preset: EncodePreset,
    udp_egress: Option<String>,
    pipeline: Option<gstreamer::Pipeline>,
    /// Dynamic proxy REC branch (always on encoded tee `e`).
    proxy_branch: Option<Branch>,
    /// Dynamic HQ REC branch (tee `t` for mezz, tee `e` for encoded).
    hq_branch: Option<Branch>,
    /// On-demand WebRTC encode preview (modal). At most one per channel.
    webrtc_preview: Option<crate::gst::preview_webrtc::WebRtcPreview>,
    /// Avoid relaunch storms: last adapt attempt.
    last_adapt: Option<std::time::Instant>,
    /// Last p↔i mode flip while waiting for signal (auto only).
    last_signal_flip: Option<std::time::Instant>,
    /// How many p↔i flips while waiting since last Running (cap to avoid relaunch storms).
    signal_flip_attempts: u8,
    /// Format-adapt hysteresis: mode must stay stable before relaunch.
    pending_adapt_mode: Option<(String, std::time::Instant)>,
    /// Peak dBFS per discrete channel (8). Updated from `level` bus messages.
    audio_peaks: [f64; 8],
    /// Last time a `level` bus message updated `audio_peaks`.
    last_level_at: Option<std::time::Instant>,
    /// Live encoded bitstream rate (tee `e` sink probe).
    encode_bitrate: BitrateMeter,
    /// Live SRT MPEG-TS rate (appsrc → srtsink), only while SRT is enabled.
    srt_bitrate: Option<BitrateMeter>,
    /// Incremented on each successful `launch_locked` (SRT/preset/adapt relaunch).
    preview_epoch: u64,
}

struct Branch {
    role: RecordingRole,
    /// Name of the video tee this branch's `tee_pad` was requested from (`"e"` | `"t"`).
    /// Detach must release the pad on this exact tee.
    video_tee: &'static str,
    /// Request pad on `video_tee`; `None` while a branch is only partially built.
    tee_pad: Option<gstreamer::Pad>,
    /// Pads from audio tee `a` (one per AAC stereo pair).
    audio_tee_pads: Vec<gstreamer::Pad>,
    elements: Vec<gstreamer::Element>,
}

/// In-flight REC finalize: EOS wait can run outside the global `gst_op` lock.
pub struct PendingRecordDetach {
    channel_id: u32,
    tag: String,
    pipeline: gstreamer::Pipeline,
    branch: Branch,
    rx: std::sync::mpsc::Receiver<()>,
    /// Keep probe ids until finish so probes stay installed during wait.
    eos_probe: Option<gstreamer::PadProbeId>,
    block_probes: Vec<gstreamer::PadProbeId>,
}

impl PendingRecordDetach {
    /// Wait for filesink EOS. On timeout returns `Err` (caller should still `finish`).
    pub fn wait_eos(&self) -> Result<()> {
        // ProRes/qtmux drain over NFS can exceed 15s after a long encode.
        match self.rx.recv_timeout(std::time::Duration::from_millis(45000)) {
            Ok(()) => {
                tracing::info!(
                    channel = self.channel_id,
                    role = %self.tag,
                    "record EOS reached filesink"
                );
                Ok(())
            }
            Err(_) => {
                tracing::warn!(
                    channel = self.channel_id,
                    role = %self.tag,
                    "record EOS timeout — moov may be missing"
                );
                Err(anyhow!(
                    "record EOS timeout on channel {} role {} — file may lack moov/footer",
                    self.channel_id,
                    self.tag
                ))
            }
        }
    }

    pub fn finish(self) {
        drop(self.eos_probe);
        drop(self.block_probes);
        ChannelPipeline::unlink_branch(&self.pipeline, &self.branch);
        for el in self.branch.elements.iter().rev() {
            let _ = el.set_state(gstreamer::State::Null);
        }
        for el in &self.branch.elements {
            let _ = self.pipeline.remove(el);
        }
        tracing::info!(
            channel = self.channel_id,
            role = %self.tag,
            video_tee = self.branch.video_tee,
            "detached record branch"
        );
    }
}

/// Parser for the NVENC bitstream on encoded tee `e` (follows the *encode* preset).
fn parse_element_for_codec(video_codec: &str) -> &'static str {
    let c = video_codec.to_ascii_lowercase();
    if c.contains("265") || c.contains("hevc") {
        "h265parse"
    } else {
        // H.264 (and any unknown/mezz codec, which never reaches tee `e`).
        "h264parse"
    }
}

impl ChannelPipeline {
    pub fn new(ch: &ChannelConfig, encode: &EncodePreset, record: &EncodePreset) -> Result<Self> {
        let encode_id = ch
            .encode_preset
            .clone()
            .unwrap_or_else(|| "hq".into());
        let record_id = ch
            .record_preset
            .clone()
            .unwrap_or_else(|| encode_id.clone());
        Ok(Self {
            id: ch.id,
            name: ch.name.clone(),
            encode_preset_id: encode_id,
            encode_preset_label: encode.label.clone(),
            record_preset_id: record_id,
            record_preset_label: record.label.clone(),
            status: ChannelStatus::Stopped,
            proxy_recording: false,
            hq_recording: false,
            srt: false,
            proxy_recording_path: None,
            hq_recording_path: None,
            srt_url: ch.srt_url.clone(),
            last_error: None,
            device: ch.device.clone(),
            configured_mode: ch
                .mode
                .clone()
                .unwrap_or_else(|| "auto".into()),
            locked_mode: String::new(),
            detected: None,
            preset: encode.clone(),
            record_preset: record.clone(),
            udp_egress: ch.udp_egress.clone(),
            pipeline: None,
            proxy_branch: None,
            hq_branch: None,
            webrtc_preview: None,
            last_adapt: None,
            last_signal_flip: None,
            signal_flip_attempts: 0,
            pending_adapt_mode: None,
            audio_peaks: [-90.0; 8],
            last_level_at: None,
            encode_bitrate: BitrateMeter::new(),
            srt_bitrate: None,
            preview_epoch: 0,
        })
    }

    pub fn update_config(
        &mut self,
        ch: &ChannelConfig,
        encode: &EncodePreset,
        record: &EncodePreset,
    ) {
        self.name = ch.name.clone();
        self.device = ch.device.clone();
        self.configured_mode = ch.mode.clone().unwrap_or_else(|| "auto".into());
        self.preset = encode.clone();
        self.record_preset = record.clone();
        if let Some(id) = &ch.encode_preset {
            self.encode_preset_id = id.clone();
        }
        self.encode_preset_label = encode.label.clone();
        if let Some(id) = &ch.record_preset {
            self.record_preset_id = id.clone();
        } else if let Some(id) = &ch.encode_preset {
            self.record_preset_id = id.clone();
        }
        self.record_preset_label = record.label.clone();
        self.udp_egress = ch.udp_egress.clone();
        if self.srt_url.is_none() {
            self.srt_url = ch.srt_url.clone();
        }
    }

    /// Apply a new live/proxy encode preset. Relunches when live (preserves REC/SRT).
    pub fn apply_encode_preset(&mut self, preset_id: &str, preset: &EncodePreset) -> Result<()> {
        if roc_config::is_mezz_codec(&preset.video_codec) {
            bail!("mezz codecs belong on the record preset — pick an NVENC proxy for live/SRT");
        }
        // Relaunch rebuilds tee `e`; any active REC (proxy or HQ) must be stopped first.
        if self.proxy_recording || self.hq_recording {
            bail!("stop recording before changing encode preset");
        }
        let live = matches!(
            self.status,
            ChannelStatus::Running | ChannelStatus::Waiting
        );
        self.encode_preset_id = preset_id.to_string();
        self.encode_preset_label = preset.label.clone();
        self.preset = preset.clone();
        if !live {
            tracing::info!(
                channel = self.id,
                preset_id,
                "encode (proxy) preset updated (applies on next start)"
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
            "encode (proxy) preset changed — relaunching capture"
        );
        self.relaunch_preserving_branches(&mode)
    }

    /// Apply REC preset. No graph relaunch; used on next HQ start. Only an active
    /// HQ recording blocks the change — proxy REC follows the encode preset and
    /// can keep running.
    pub fn apply_record_preset(&mut self, preset_id: &str, preset: &EncodePreset) -> Result<()> {
        if self.hq_recording {
            bail!("stop recording before changing record preset");
        }
        self.record_preset_id = preset_id.to_string();
        self.record_preset_label = preset.label.clone();
        self.record_preset = preset.clone();
        tracing::info!(
            channel = self.id,
            preset_id,
            codec = %preset.video_codec,
            "record preset updated"
        );
        Ok(())
    }

    fn relaunch_preserving_branches(&mut self, mode: &str) -> Result<()> {
        // Remember which roles were active so both can be re-attached after relaunch.
        let proxy_path = if self.proxy_recording {
            self.proxy_recording_path.clone()
        } else {
            None
        };
        let hq_path = if self.hq_recording {
            self.hq_recording_path.clone()
        } else {
            None
        };
        // Keep self.srt / self.srt_url — launch_locked bakes SRT into the graph.

        self.dispose_webrtc_preview();
        let _ = self.detach_recording(RecordingRole::Proxy, false);
        let _ = self.detach_recording(RecordingRole::Hq, false);
        if let Some(p) = self.pipeline.take() {
            let _ = p.set_state(gstreamer::State::Null);
        }
        self.proxy_recording = false;
        self.hq_recording = false;
        self.proxy_recording_path = None;
        self.hq_recording_path = None;
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
                if let Some(path) = proxy_path {
                    if let Err(err) = self.start_proxy_recording(&path) {
                        tracing::error!(
                            channel = self.id,
                            error = %err,
                            "failed to re-attach proxy recording after relaunch"
                        );
                    }
                }
                if let Some(path) = hq_path {
                    if let Err(err) = self.start_hq_recording(&path) {
                        tracing::error!(
                            channel = self.id,
                            error = %err,
                            "failed to re-attach HQ recording after relaunch"
                        );
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

    /// Start with a pre-resolved DeckLink mode (probe done outside `gst_op`).
    pub fn start_with_mode(&mut self, locked: &str) -> Result<()> {
        if self.pipeline.is_some() {
            return Ok(());
        }
        self.locked_mode = locked.to_string();
        self.launch_locked(locked)?;
        Ok(())
    }

    pub fn device_name(&self) -> &str {
        &self.device
    }

    pub fn configured_mode(&self) -> &str {
        &self.configured_mode
    }

    pub fn set_detected(&mut self, fmt: InputFormat) {
        self.detected = Some(fmt);
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
                    || n.starts_with("thumb")
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
                if let Err(err) = arm_srt_appsink_gate(
                    self.id,
                    &pipeline,
                    &uri,
                    aac_stereo_pairs(self.preset.audio_channels),
                ) {
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
        // Grace period before p↔i "no signal" flip. Cold start used to leave
        // `last_adapt` unset so the first poll flipped immediately while DeckLink
        // `signal` was still false — locking the wrong scan and starving REC.
        self.last_adapt = Some(std::time::Instant::now());
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        self.dispose_webrtc_preview();
        let _ = self.detach_recording(RecordingRole::Proxy, false);
        let _ = self.detach_recording(RecordingRole::Hq, false);
        if let Some(p) = self.pipeline.take() {
            let _ = p.send_event(gstreamer::event::Eos::new());
            let _ = p.set_state(gstreamer::State::Null);
        }
        self.status = ChannelStatus::Stopped;
        self.proxy_recording = false;
        self.hq_recording = false;
        self.srt = false;
        self.proxy_recording_path = None;
        self.hq_recording_path = None;
        self.audio_peaks = [-90.0; 8];
        self.last_level_at = None;
        self.encode_bitrate.reset();
        self.srt_bitrate = None;
        Ok(())
    }

    /// Proxy REC: encoded tee `e` → encode-preset parser → mp4mux (.mp4).
    pub fn start_proxy_recording(&mut self, path: &str) -> Result<()> {
        if self.pipeline.is_none() {
            bail!("capture not running");
        }
        if self.proxy_recording {
            bail!("already recording proxy");
        }
        // Encoded HQ already runs its own voaacenc chain from tee `a`.
        if self.hq_recording && !roc_config::is_mezz_codec(&self.record_preset.video_codec) {
            bail!(
                "stop encoded HQ recording before starting proxy \
                 (dual encoded REC would double AAC encode); use a mezz HQ preset instead"
            );
        }
        let branch = self.attach_encoded_recording(RecordingRole::Proxy, path)?;
        self.proxy_branch = Some(branch);
        self.proxy_recording = true;
        self.proxy_recording_path = Some(path.to_string());
        Ok(())
    }

    /// Begin proxy REC stop (inject EOS). Caller must `wait_eos` then `finish`
    /// — preferably outside the global `gst_op` lock.
    pub fn begin_stop_proxy_recording(&mut self) -> Result<Option<PendingRecordDetach>> {
        if !self.proxy_recording {
            return Ok(None);
        }
        self.proxy_recording = false;
        self.proxy_recording_path = None;
        self.begin_detach_recording(RecordingRole::Proxy, true)
    }

    /// HQ REC: mezz from raw tee `t` when the record preset is mezz, otherwise
    /// the encoded bitstream from tee `e`.
    pub fn start_hq_recording(&mut self, path: &str) -> Result<()> {
        if self.pipeline.is_none() {
            bail!("capture not running");
        }
        if self.hq_recording {
            bail!("already recording hq");
        }
        let mezz = roc_config::is_mezz_codec(&self.record_preset.video_codec);
        // Proxy already owns an AAC encode from tee `a`; a second encoded HQ
        // would run another full voaacenc bank (8ch → 4× encoders).
        if self.proxy_recording && !mezz {
            bail!(
                "stop proxy before starting encoded HQ recording \
                 (dual encoded REC would double AAC encode); use a mezz HQ preset instead"
            );
        }
        let branch = if mezz {
            self.attach_mezz_recording(RecordingRole::Hq, path)?
        } else {
            self.attach_encoded_recording(RecordingRole::Hq, path)?
        };
        self.hq_branch = Some(branch);
        self.hq_recording = true;
        self.hq_recording_path = Some(path.to_string());
        Ok(())
    }

    /// Begin HQ REC stop (inject EOS). Caller must `wait_eos` then `finish`
    /// — preferably outside the global `gst_op` lock.
    pub fn begin_stop_hq_recording(&mut self) -> Result<Option<PendingRecordDetach>> {
        if !self.hq_recording {
            return Ok(None);
        }
        self.hq_recording = false;
        self.hq_recording_path = None;
        self.begin_detach_recording(RecordingRole::Hq, true)
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
        // Wait for all program AAC ES (4 when preset is 8ch) before opening the
        // MediaMTX gate, otherwise receivers lock onto a stereo-only first PMT.
        disarm_srt_appsink_gate(self.id, &pipeline);
        arm_srt_appsink_gate(
            self.id,
            &pipeline,
            &gst_url,
            aac_stereo_pairs(self.preset.audio_channels),
        )
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

    /// Dispose any parked bin and attach a fresh WebRTC branch (no SDP offer yet).
    /// Caller must [`Self::emit_webrtc_offer`] outside `gst_op`.
    pub fn attach_webrtc_preview(
        &mut self,
        pair: u8,
        signal_tx: crate::PreviewSignalTx,
    ) -> Result<String> {
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow!("capture not running"))?
            .clone();
        // Always dispose then attach: parked webrtcbin ICE/DTLS state is unreliable
        // across a new browser PeerConnection. Soft-park still protects meters while closed.
        if let Some(old) = self.webrtc_preview.take() {
            old.dispose(&pipeline);
        }
        let preview = crate::gst::preview_webrtc::WebRtcPreview::attach(
            &pipeline,
            self.id,
            pair,
            signal_tx,
        )?;
        let sid = preview.session_id.clone();
        self.webrtc_preview = Some(preview);
        Ok(sid)
    }

    /// SDP offer — may block ~5s; call outside the global `gst_op` lock.
    pub fn emit_webrtc_offer(&self, signal_tx: crate::PreviewSignalTx) -> Result<()> {
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow!("capture not running"))?;
        let preview = self
            .webrtc_preview
            .as_ref()
            .ok_or_else(|| anyhow!("no webrtc preview session"))?;
        preview.emit_offer(pipeline, signal_tx)
    }

    pub fn set_webrtc_answer(&self, sdp: &str) -> Result<()> {
        let Some(p) = self.webrtc_preview.as_ref() else {
            bail!("no webrtc preview session");
        };
        p.set_remote_answer(sdp)
    }

    pub fn add_webrtc_ice(&self, sdp_mline_index: u32, candidate: &str) -> Result<()> {
        let Some(p) = self.webrtc_preview.as_ref() else {
            bail!("no webrtc preview session");
        };
        p.add_ice_candidate(sdp_mline_index, candidate);
        Ok(())
    }

    /// Soft-close: park valves; next open disposes + attaches a fresh bin.
    pub fn stop_webrtc_preview(&mut self) {
        let Some(preview) = self.webrtc_preview.as_mut() else {
            return;
        };
        if let Some(pipeline) = self.pipeline.as_ref() {
            preview.park(pipeline);
        }
    }

    /// Hard remove (capture stop). Restores valves and Nulls the bin.
    pub fn dispose_webrtc_preview(&mut self) {
        let Some(preview) = self.webrtc_preview.take() else {
            return;
        };
        if let Some(pipeline) = self.pipeline.as_ref() {
            preview.dispose(pipeline);
        }
    }

    /// Stop preview only if `session_id` is still the active one (pop-out handoff safe).
    pub fn stop_webrtc_preview_session(&mut self, session_id: &str) {
        let Some(preview) = self.webrtc_preview.as_ref() else {
            return;
        };
        if preview.session_id != session_id {
            return;
        }
        self.stop_webrtc_preview();
    }

    fn encoded_tee(&self) -> Result<gstreamer::Element> {
        let p = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow!("no pipeline"))?;
        p.by_name("e")
            .ok_or_else(|| anyhow!("encoded tee `e` missing — is capture running?"))
    }

    /// Video tee for mezz REC.
    /// DNxHD keeps interlaced fields from pre-deinterlace `raw`.
    /// ProRes uses progressive post-deinterlace `t` (QuickTime-friendly).
    fn mezz_video_tee(&self) -> Result<gstreamer::Element> {
        let p = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow!("no pipeline"))?;
        let codec = self.record_preset.video_codec.to_ascii_lowercase();
        if codec.contains("prores") {
            return p
                .by_name("t")
                .ok_or_else(|| anyhow!("progressive tee `t` missing — is capture running?"));
        }
        p.by_name("raw")
            .or_else(|| p.by_name("t"))
            .ok_or_else(|| anyhow!("mezz video tee `raw`/`t` missing — is capture running?"))
    }

    /// Best-effort bit depth from live DeckLink caps (8 if unknown).
    fn source_bit_depth(&self) -> u8 {
        if let Some(fmt) = self.detected.as_ref() {
            if fmt.bit_depth >= 8 {
                return fmt.bit_depth;
            }
        }
        let Some(pipeline) = self.pipeline.as_ref() else {
            return 8;
        };
        let Some(src) = pipeline.by_name("dlsrc") else {
            return 8;
        };
        let Some(pad) = src.static_pad("src") else {
            return 8;
        };
        let Some(caps) = pad.current_caps().or_else(|| pad.allowed_caps()) else {
            return 8;
        };
        let Some(s) = caps.structure(0) else {
            return 8;
        };
        if let Ok(depth) = s.get::<i32>("bit-depth-luma") {
            if depth >= 10 {
                return 10;
            }
            if depth > 0 {
                return depth as u8;
            }
        }
        if let Ok(fmt) = s.get::<&str>("format") {
            let f = fmt.to_ascii_uppercase();
            // Common 10-bit DeckLink / raw formats.
            if f.contains("V210") || f.contains("R210") || f.contains("P010") || f.contains("Y210")
            {
                return 10;
            }
        }
        8
    }

    /// Preview label for the HQ mezz OP given live signal + record preset (e.g. `DNxHD 185`).
    fn mezz_label_for_signal(&self) -> Option<String> {
        let fmt = self.detected.as_ref()?;
        let codec = self.record_preset.video_codec.to_ascii_lowercase();
        if codec.contains("dnx") {
            let hint = crate::parse_bitrate(&self.record_preset.video_bitrate);
            let class_src = if !self.record_preset.video_preset.trim().is_empty() {
                self.record_preset.video_preset.as_str()
            } else {
                self.record_preset.label.as_str()
            };
            let class = crate::DnxhdClass::parse(class_src, hint);
            return crate::resolve_dnxhd(fmt, class)
                .ok()
                .map(|op| op.label);
        }
        if codec.contains("prores") {
            let profile = roc_config::parse_prores_profile(&self.record_preset.video_preset);
            let label = match profile {
                "proxy" => "ProRes Proxy",
                "lt" => "ProRes LT",
                "hq" => "ProRes HQ",
                "4444" | "4444xq" => "ProRes 4444",
                _ => "ProRes 422",
            };
            return Some(label.into());
        }
        None
    }

    /// Audio settings for a REC role. Proxy follows the live/proxy encode preset;
    /// HQ follows the record preset.
    fn audio_preset_for(&self, role: RecordingRole) -> &EncodePreset {
        match role {
            RecordingRole::Proxy => &self.preset,
            RecordingRole::Hq => &self.record_preset,
        }
    }

    /// Release tee pads and remove every element of a (possibly partial) branch.
    /// Used on attach failure and by `detach_recording`.
    fn discard_branch(pipeline: &gstreamer::Pipeline, branch: &Branch) {
        Self::unlink_branch(pipeline, branch);
        for el in branch.elements.iter().rev() {
            let _ = el.set_state(gstreamer::State::Null);
        }
        for el in &branch.elements {
            let _ = pipeline.remove(el);
        }
    }

    /// NVENC bitstream REC from encoded tee `e` → parse → mp4mux.
    ///
    /// Used by the proxy role (always) and by the HQ role when the record preset
    /// is not a mezz codec. The parser follows the *encode* preset codec because
    /// that is what tee `e` carries.
    fn attach_encoded_recording(&self, role: RecordingRole, path: &str) -> Result<Branch> {
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow!("no pipeline"))?
            .clone();
        let mut branch = Branch {
            role,
            video_tee: "e",
            tee_pad: None,
            audio_tee_pads: Vec::new(),
            elements: Vec::new(),
        };
        match self.build_encoded_recording(&pipeline, &mut branch, path) {
            Ok(()) => Ok(branch),
            Err(err) => {
                tracing::error!(
                    channel = self.id,
                    role = role.tag(),
                    error = %err,
                    "encoded record attach failed — discarding partial branch"
                );
                Self::discard_branch(&pipeline, &branch);
                Err(err)
            }
        }
    }

    fn build_encoded_recording(
        &self,
        pipeline: &gstreamer::Pipeline,
        branch: &mut Branch,
        path: &str,
    ) -> Result<()> {
        let role = branch.role;
        let tag = role.tag();
        let tee = self.encoded_tee()?;
        let audio_tee = pipeline.by_name("a");
        let parse_element = parse_element_for_codec(&self.preset.video_codec);

        let queue_v = gstreamer::ElementFactory::make("queue")
            .name(format!("q_{tag}_v_{}", self.id))
            .build()
            .context("queue video")?;
        let parse = gstreamer::ElementFactory::make(parse_element)
            .name(format!("parse_{tag}_{}", self.id))
            .build()
            .with_context(|| format!("make {parse_element}"))?;
        // Keep SPS/PPS (or VPS/SPS/PPS for HEVC) in-band for mid-stream joiners.
        let _ = parse.set_property_from_str("config-interval", "-1");
        // Progressive MP4 (moov at end). Fragmented/streamable files often look
        // "corrupt" in browsers and desktop players when finalize is incomplete.
        let mux = gstreamer::ElementFactory::make("mp4mux")
            .name(format!("mux_{tag}_{}", self.id))
            .property("fragment-duration", 0u32)
            .property("streamable", false)
            .build()
            .context("mp4mux")?;
        let sink = gstreamer::ElementFactory::make("filesink")
            .name(format!("fs_{tag}_{}", self.id))
            .property("location", path)
            .property("sync", false)
            // Ensure CIFS/NFS sees bytes promptly; helps avoid 0-byte stubs on stop.
            .property("async", false)
            .build()
            .context("filesink")?;

        let id_v = make_mux_ts_align(&format!("id_{tag}_v_{}", self.id))?;
        // First element must stay the video queue (`unlink_branch` relies on it).
        branch.elements.extend([
            queue_v.clone(),
            parse.clone(),
            id_v.clone(),
            mux.clone(),
            sink.clone(),
        ]);
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
        let audio_queue_prefix = format!("q_{tag}_a");
        if let Some(a_tee) = audio_tee {
            match self.link_program_aac(
                pipeline,
                &a_tee,
                &mux,
                tag,
                true,
                self.audio_preset_for(role),
            ) {
                Ok((a_pads, audio_els)) => {
                    // Gate every AAC queue until the first video keyframe.
                    for el in &audio_els {
                        if el.name().starts_with(&audio_queue_prefix) {
                            install_av_start_gate(el, av_gate.clone());
                        }
                    }
                    branch.audio_tee_pads = a_pads;
                    branch.elements.extend(audio_els);
                    install_mux_av_sync_log(tag);
                }
                Err(err) => {
                    tracing::warn!(
                        channel = self.id,
                        role = tag,
                        error = %err,
                        "REC video-only — program AAC attach failed"
                    );
                }
            }
        } else {
            tracing::warn!(
                channel = self.id,
                role = tag,
                "REC video-only — audio tee `a` missing"
            );
        }

        for el in &branch.elements {
            el.sync_state_with_parent()
                .context("sync_state_with_parent record")?;
        }

        // Open the audio gate on the first video keyframe (non-DELTA_UNIT).
        if !branch.audio_tee_pads.is_empty() {
            arm_av_gate_on_keyframe(&id_v, av_gate);
        }

        let tee_pad = tee
            .request_pad_simple("src_%u")
            .ok_or_else(|| anyhow!("tee request_pad failed"))?;
        // Record the pad before linking so a link failure still releases it.
        branch.tee_pad = Some(tee_pad.clone());
        let sink_pad = queue_v
            .static_pad("sink")
            .ok_or_else(|| anyhow!("queue sink pad"))?;
        tee_pad
            .link(&sink_pad)
            .context("link tee → record queue")?;

        tracing::info!(
            channel = self.id,
            role = tag,
            %path,
            parse = parse_element,
            video_tee = branch.video_tee,
            audio_pairs = branch.audio_tee_pads.len(),
            "attached record branch (no relaunch)"
        );
        Ok(())
    }

    /// Mezz REC: DNxHD (.mxf from `raw`) or ProRes (.mov from `t`) + NTP/RTC timecode.
    ///
    /// HQ-only. Detach releases the mezz tee pad (not encoded tee `e`).
    fn attach_mezz_recording(&self, role: RecordingRole, path: &str) -> Result<Branch> {
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow!("no pipeline"))?
            .clone();
        let mut branch = Branch {
            role,
            video_tee: "raw",
            tee_pad: None,
            audio_tee_pads: Vec::new(),
            elements: Vec::new(),
        };
        match self.build_mezz_recording(&pipeline, &mut branch, path) {
            Ok(()) => Ok(branch),
            Err(err) => {
                tracing::error!(
                    channel = self.id,
                    role = role.tag(),
                    error = %err,
                    "mezz record attach failed — discarding partial branch"
                );
                Self::discard_branch(&pipeline, &branch);
                Err(err)
            }
        }
    }

    fn build_mezz_recording(
        &self,
        pipeline: &gstreamer::Pipeline,
        branch: &mut Branch,
        path: &str,
    ) -> Result<()> {
        let tag = branch.role.tag();
        let tee = self.mezz_video_tee()?;
        branch.video_tee = if tee.name() == "raw" { "raw" } else { "t" };
        let audio_tee = pipeline.by_name("a");
        let codec = self.record_preset.video_codec.to_ascii_lowercase();

        let fmt = self.detected.clone().ok_or_else(|| {
            anyhow!("no detected input format — wait for signal before DNxHD/ProRes REC")
        })?;

        tracing::warn!(
            channel = self.id,
            codec = %self.record_preset.video_codec,
            "mezz REC shares the live graph — CPU encode may drop frames \
             (leaky 2s queue protects proxy/WebRTC; prefer Proxy/LT ProRes or DNxHD)"
        );
        let queue_v = gstreamer::ElementFactory::make("queue")
            .name(format!("q_{tag}_v_{}", self.id))
            // Isolate mezz from live tee. Prefer absorbing NFS/encode jitter
            // over wedging proxy/WebRTC; leaky drops mezz frames only under stall.
            .property("max-size-buffers", 60u32)
            .property("max-size-bytes", 0u32)
            .property("max-size-time", gstreamer::ClockTime::from_seconds(2))
            .build()
            .context("mezz queue")?;
        let _ = queue_v.set_property_from_str("leaky", "downstream");
        let convert = gstreamer::ElementFactory::make("videoconvert")
            .name(format!("vconv_{tag}_{}", self.id))
            .build()
            .context("videoconvert")?;
        // DeckLink may expose i50 as 50/1 fields; DNxHD OP wants 25/1 frames.
        let rate = gstreamer::ElementFactory::make("videorate")
            .name(format!("vrate_{tag}_{}", self.id))
            .property("skip-to-first", true)
            .build()
            .context("videorate")?;

        let (caps_str, bitrate, enc_label, frame_duration_ns, prores_profile) =
            if codec.contains("dnx") {
                let hint = crate::parse_bitrate(&self.record_preset.video_bitrate);
                let class_src = if !self.record_preset.video_preset.trim().is_empty() {
                    self.record_preset.video_preset.as_str()
                } else {
                    self.record_preset.label.as_str()
                };
                let class = crate::DnxhdClass::parse(class_src, hint);
                let op = crate::resolve_dnxhd(&fmt, class)?;
                if class == crate::DnxhdClass::Hqx && self.source_bit_depth() < 10 {
                    bail!(
                        "DNxHD HQX requires a 10-bit source (signal is {}-bit). \
                         Use DNxHD SQ/HQ (8-bit) or feed a 10-bit input.",
                        self.source_bit_depth()
                    );
                }
                if class.bits() == 8 && self.source_bit_depth() >= 10 {
                    tracing::warn!(
                        channel = self.id,
                        class = class.as_str(),
                        source_bits = self.source_bit_depth(),
                        "DNxHD {}: 10-bit source will be recorded as 8-bit (Y42B). Use HQX to keep 10-bit.",
                        class.as_str()
                    );
                }
                let frame_duration_ns = 1_000_000_000u64
                    .saturating_mul(op.fps_den as u64)
                    .saturating_div(op.fps_num as u64)
                    .max(1);
                tracing::info!(
                    channel = self.id,
                    class = class.as_str(),
                    label = %op.label,
                    bitrate = op.bitrate,
                    interlaced = op.interlaced,
                    fps = %format!("{}/{}", op.fps_num, op.fps_den),
                    raw_format = op.raw_format,
                    source_bits = self.source_bit_depth(),
                    "DNxHD operating point resolved from live signal"
                );
                (
                    op.video_caps(),
                    op.bitrate,
                    op.label,
                    frame_duration_ns,
                    None,
                )
            } else if codec.contains("prores") {
                let profile = roc_config::parse_prores_profile(
                    if !self.record_preset.video_preset.trim().is_empty() {
                        self.record_preset.video_preset.as_str()
                    } else {
                        self.record_preset.label.as_str()
                    },
                );
                let (fps_n, fps_d) = if fmt.interlaced {
                    if fmt.fps_num >= 40 {
                        (fmt.fps_num / 2, fmt.fps_den.max(1))
                    } else {
                        (fmt.fps_num.max(1), fmt.fps_den.max(1))
                    }
                } else {
                    (fmt.fps_num.max(1), fmt.fps_den.max(1))
                };
                let frame_duration_ns = 1_000_000_000u64
                    .saturating_mul(fps_d as u64)
                    .saturating_div(fps_n as u64)
                    .max(1);
                let raw_format = if profile.starts_with("4444") {
                    "Y444_10LE"
                } else {
                    "I422_10LE"
                };
                let caps = format!("video/x-raw,format={raw_format},framerate={fps_n}/{fps_d}");
                let enc_label = match profile {
                    "proxy" => "ProRes Proxy",
                    "lt" => "ProRes LT",
                    "hq" => "ProRes HQ",
                    "4444" | "4444xq" => "ProRes 4444",
                    _ => "ProRes 422",
                }
                .to_string();
                let bitrate = crate::parse_bitrate(&self.record_preset.video_bitrate)
                    .unwrap_or(147_000_000);
                tracing::info!(
                    channel = self.id,
                    profile,
                    label = %enc_label,
                    fps = %format!("{fps_n}/{fps_d}"),
                    raw_format,
                    "ProRes profile resolved"
                );
                (caps, bitrate, enc_label, frame_duration_ns, Some(profile))
            } else {
                bail!("unsupported mezz codec {}", self.record_preset.video_codec);
            };

        let caps = gstreamer::ElementFactory::make("capsfilter")
            .name(format!("caps_{tag}_{}", self.id))
            .property(
                "caps",
                gstreamer::Caps::from_str(&caps_str).context("mezz video caps")?,
            )
            .build()
            .context("capsfilter")?;
        // RTC source uses the host clock (chrony/NTP). No PTP on this machine.
        let tc = gstreamer::ElementFactory::make("timecodestamper")
            .name(format!("tc_{tag}_{}", self.id))
            .build()
            .context("timecodestamper")?;
        let _ = tc.set_property_from_str("source", "rtc");
        let _ = tc.set_property_from_str("set", "always");

        let (enc, mux_name): (gstreamer::Element, &str) = if codec.contains("dnx") {
            let enc = gstreamer::ElementFactory::make("avenc_dnxhd")
                .name(format!("enc_{tag}_{}", self.id))
                .property("bitrate", bitrate as i32)
                .build()
                .context("avenc_dnxhd")?;
            let _ = enc.set_property_from_str("profile", "dnxhd");
            // Host probe: only mxfmux accepts video/x-dnxhd (qtmux/avmux_mov do not).
            (enc, "mxfmux")
        } else if codec.contains("prores") {
            let enc = gstreamer::ElementFactory::make("avenc_prores_ks")
                .name(format!("enc_{tag}_{}", self.id))
                .build()
                .context("avenc_prores_ks")?;
            let profile = prores_profile.unwrap_or("standard");
            let _ = enc.set_property_from_str("profile", profile);
            let _ = bitrate; // ProRes is profile-driven; stored Mbps is UI-only.
            (enc, "qtmux")
        } else {
            bail!("unsupported mezz codec {}", self.record_preset.video_codec);
        };

        // Hot-attach onto a live tee: reset TIME segment *after* encode so
        // mxfmux sees pts≈0 (raw-side identity alone is not enough for avenc).
        let id_v = make_mux_ts_align(&format!("id_{tag}_v_{}", self.id))?;

        let mux = gstreamer::ElementFactory::make(mux_name)
            .name(format!("mux_{tag}_{}", self.id))
            .build()
            .with_context(|| format!("make {mux_name}"))?;
        if mux_name == "qtmux" {
            // Keep a playable moov even if final EOS is slow/missed (ProRes drain).
            // Cap reservation short: a 2h window pre-allocated tens of MB on NFS and
            // stalled filesink → encoder blocked → ~1s of frames then EOS timeout.
            let _ = mux.set_property(
                "reserved-max-duration",
                gstreamer::ClockTime::from_seconds(15 * 60),
            );
            let _ = mux.set_property(
                "reserved-moov-update-period",
                gstreamer::ClockTime::from_seconds(2),
            );
        }
        let sink = gstreamer::ElementFactory::make("filesink")
            .name(format!("fs_{tag}_{}", self.id))
            .property("location", path)
            .property("sync", false)
            .property("async", false)
            .build()
            .context("filesink")?;

        // First element must stay the video queue (`unlink_branch` relies on it).
        branch.elements.extend([
            queue_v.clone(),
            convert.clone(),
            rate.clone(),
            caps.clone(),
            tc.clone(),
            enc.clone(),
            id_v.clone(),
            mux.clone(),
            sink.clone(),
        ]);
        pipeline.add_many([
            &queue_v, &convert, &rate, &caps, &tc, &enc, &id_v, &mux, &sink,
        ])?;
        queue_v.link(&convert).context("mezz queue→convert")?;
        convert.link(&rate).context("mezz convert→videorate")?;
        rate.link(&caps).context("mezz videorate→caps")?;
        caps.link(&tc).context("mezz caps→timecode")?;
        tc.link(&enc).context("mezz timecode→enc")?;
        enc.link(&id_v).context("mezz enc→identity")?;
        id_v.link(&mux).context("mezz identity→mux")?;
        mux.link(&sink).context("mezz mux→sink")?;

        // Gate PCM until the first video buffer reaches post-encode identity so
        // mxfmux does not open on audio alone with a huge running PTS.
        let av_gate = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        // ProRes/qtmux: publish video PTS so PCM can be paced behind a slow encode.
        let pace_audio = codec.contains("prores");
        let video_pts_ns = if pace_audio {
            // u64::MAX = no video frame yet (0 is a valid first PTS).
            Some(std::sync::Arc::new(std::sync::atomic::AtomicU64::new(
                u64::MAX,
            )))
        } else {
            None
        };
        let audio_queue_prefix = format!("q_{tag}_pcm");
        if let Some(a_tee) = audio_tee {
            match self.link_mezz_pcm(pipeline, &a_tee, &mux, tag, video_pts_ns.clone()) {
                Ok((a_pads, audio_els)) => {
                    for el in &audio_els {
                        if el.name().starts_with(&audio_queue_prefix) {
                            install_av_start_gate(el, av_gate.clone());
                        }
                    }
                    branch.audio_tee_pads = a_pads;
                    branch.elements.extend(audio_els);
                    install_mux_av_sync_log("mezz");
                }
                Err(err) => {
                    tracing::warn!(
                        channel = self.id,
                        role = tag,
                        error = %err,
                        "mezz REC video-only — audio attach failed"
                    );
                }
            }
        }

        for el in &branch.elements {
            el.sync_state_with_parent()
                .context("sync_state_with_parent mezz record")?;
        }

        // Force CFR timestamps into mxfmux/qtmux (hot-attach skips a fresh segment).
        arm_mezz_pts_reset(&id_v, frame_duration_ns, video_pts_ns);

        if !branch.audio_tee_pads.is_empty() {
            // Intra codecs: every frame is a keyframe — first buffer opens the gate.
            arm_av_gate_on_keyframe(&id_v, av_gate);
        }

        let tee_pad = tee
            .request_pad_simple("src_%u")
            .ok_or_else(|| anyhow!("mezz video tee request_pad failed"))?;
        // Record the pad before linking so a link failure still releases it.
        branch.tee_pad = Some(tee_pad.clone());
        let sink_pad = queue_v
            .static_pad("sink")
            .ok_or_else(|| anyhow!("queue sink pad"))?;
        tee_pad
            .link(&sink_pad)
            .context("link mezz video tee → record")?;

        tracing::info!(
            channel = self.id,
            role = tag,
            %path,
            codec = %self.record_preset.video_codec,
            enc = %enc_label,
            mux = mux_name,
            bitrate,
            video_tee = branch.video_tee,
            audio_pads = branch.audio_tee_pads.len(),
            "attached mezz record branch (NTP/RTC timecode)"
        );
        Ok(())
    }

    /// PCM into mezz mux — stereo pairs from the 8ch raw tee via mix-matrix.
    /// `audio_channels >= 8` → four stereo PCM tracks (ch 1–2 … 7–8).
    fn link_mezz_pcm(
        &self,
        pipeline: &gstreamer::Pipeline,
        a_tee: &gstreamer::Element,
        mux: &gstreamer::Element,
        tag: &str,
        video_pts_ns: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
    ) -> Result<(Vec<gstreamer::Pad>, Vec<gstreamer::Element>)> {
        let pairs = if self.record_preset.audio_channels >= 8 {
            4
        } else {
            1
        };
        let mut pads = Vec::with_capacity(pairs);
        let mut els = Vec::with_capacity(pairs * 3);

        for pair in 0..pairs {
            let queue_a = gstreamer::ElementFactory::make("queue")
                .name(format!("q_{tag}_pcm{pair}_{}", self.id))
                .property("max-size-buffers", 64u32)
                .property("max-size-bytes", 0u32)
                .property("max-size-time", gstreamer::ClockTime::from_mseconds(500))
                .build()
                .context("pcm queue")?;
            let _ = queue_a.set_property_from_str("leaky", "downstream");
            let id_a = make_mux_ts_align(&format!("id_{tag}_pcm{pair}_{}", self.id))?;
            // Same parse style as AAC bins — avoid bare `format=S24LE` after mix-matrix.
            let matrix = stereo_pair_matrix(pair);
            let desc = format!(
                "audioconvert mix-matrix=\"{matrix}\" ! \
                 audio/x-raw,channels=2,rate=48000,layout=interleaved ! \
                 capsfilter caps=audio/x-raw,format=S24LE,channels=2,rate=48000,layout=interleaved"
            );
            let abin = gstreamer::parse::bin_from_description(&desc, true)
                .with_context(|| format!("mezz pcm bin pair {pair}: {desc}"))?;
            abin.set_property("name", format!("{tag}_pcm{pair}_bin_{}", self.id));
            let abin_el: gstreamer::Element = abin.upcast();

            pipeline.add_many([&queue_a, &abin_el, &id_a])?;
            queue_a.link(&abin_el).context("pcm queue→bin")?;
            abin_el.link(&id_a).context("pcm bin→identity")?;
            id_a.link(mux).context("pcm→mezz mux")?;
            // qtmux is strict about A/V timestamp domains after hot-attach.
            arm_mezz_audio_pts_reset(&id_a, video_pts_ns.clone());

            let tee_pad = a_tee
                .request_pad_simple("src_%u")
                .ok_or_else(|| anyhow!("audio tee pad"))?;
            let sink = queue_a
                .static_pad("sink")
                .ok_or_else(|| anyhow!("pcm queue sink"))?;
            tee_pad.link(&sink).context("link audio tee→pcm")?;

            pads.push(tee_pad);
            els.push(queue_a);
            els.push(abin_el);
            els.push(id_a);
        }

        Ok((pads, els))
    }

    /// Stereo AAC pair(s) into an existing mux (mp4mux / mpegtsmux).
    ///
    /// `audio_channels >= 8` → four AAC stereo pairs (same as Go/FFmpeg).
    /// `ts_align`: insert `identity single-segment` (needed for REC mp4mux late-join)
    /// and force AAC `stream-format=raw` (mp4mux rejects ADTS). Leave false for
    /// live MPEG-TS/SRT — those need ADTS and must not restamp.
    /// `audio` supplies channel count / AAC bitrate (proxy: encode preset, HQ: record preset).
    /// `tag` (`proxy` / `hq`) prefixes element names so two RECs never collide.
    fn link_program_aac(
        &self,
        pipeline: &gstreamer::Pipeline,
        a_tee: &gstreamer::Element,
        mux: &gstreamer::Element,
        tag: &str,
        ts_align: bool,
        audio: &EncodePreset,
    ) -> Result<(Vec<gstreamer::Pad>, Vec<gstreamer::Element>)> {
        let pairs = aac_stereo_pairs(audio.audio_channels);
        let hold_ms = if ts_align { 40u64 } else { 0 };
        let aac_bps = crate::parse_bitrate(&audio.audio_bitrate).unwrap_or(192_000);
        // mp4mux wants raw AAC access units; MPEG-TS (MediaMTX) wants ADTS.
        let aac_stream = if ts_align { "raw" } else { "adts" };
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
            let matrix = stereo_pair_matrix(pair);
            let desc = format!(
                "audioconvert mix-matrix=\"{matrix}\" ! \
                 audio/x-raw,channels=2 ! voaacenc bitrate={aac_bps} ! aacparse ! \
                 capsfilter caps=audio/mpeg,mpegversion=4,stream-format={aac_stream}"
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
                    .with_context(|| {
                        format!(
                            "link audio identity → mux (pair {pair}, stream-format={aac_stream})"
                        )
                    })?;
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

    /// Detach one REC role. When `finalize`, returns a pending EOS wait that
    /// the caller should run outside `gst_op`, then call [`PendingRecordDetach::finish`].
    /// When `!finalize`, unlinks immediately (relaunch / hard stop).
    fn detach_recording(&mut self, role: RecordingRole, finalize: bool) -> Result<()> {
        if let Some(pending) = self.begin_detach_recording(role, finalize)? {
            let eos = pending.wait_eos();
            pending.finish();
            eos?;
        }
        Ok(())
    }

    fn begin_detach_recording(
        &mut self,
        role: RecordingRole,
        finalize: bool,
    ) -> Result<Option<PendingRecordDetach>> {
        let taken = match role {
            RecordingRole::Proxy => self.proxy_branch.take(),
            RecordingRole::Hq => self.hq_branch.take(),
        };
        let Some(branch) = taken else {
            return Ok(None);
        };
        let tag = branch.role.tag().to_string();
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow!("no pipeline"))?
            .clone();
        let channel_id = self.id;

        if !finalize {
            ChannelPipeline::unlink_branch(&pipeline, &branch);
            for el in branch.elements.iter().rev() {
                let _ = el.set_state(gstreamer::State::Null);
            }
            for el in &branch.elements {
                let _ = pipeline.remove(el);
            }
            tracing::info!(
                channel = channel_id,
                role = %tag,
                video_tee = branch.video_tee,
                "detached record branch"
            );
            return Ok(None);
        }

        // Optional EOS for a cleaner mp4/mxf/mov footer. Do NOT wait on the
        // pipeline bus: that raced under multi-channel stop and held the global
        // lock for seconds.
        //
        // Order matters for qtmux: block live tee input, inject EOS while the
        // branch is still linked, wait for filesink, THEN unlink. Unlinking
        // first left ProRes/qtmux without a reachable EOS (moov-less .mov).
        use gstreamer::{PadProbeReturn, PadProbeType};
        use std::sync::mpsc;

        let sink_prefix = format!("fs_{tag}_");
        let queue_prefix = format!("q_{tag}_");
        let (tx, rx) = mpsc::channel::<()>();
        // Keep probe id alive until after recv — dropping it removes the probe.
        let eos_probe = branch
            .elements
            .iter()
            .find(|e| e.name().starts_with(&sink_prefix))
            .and_then(|sink| sink.static_pad("sink"))
            .and_then(|pad| {
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

        // Drop further live buffers on record queues while we finalize.
        let mut block_probes = Vec::new();
        for el in &branch.elements {
            if el.name().starts_with(&queue_prefix) {
                if let Some(pad) = el.static_pad("sink") {
                    if let Some(id) = pad.add_probe(PadProbeType::BUFFER, |_pad, _info| {
                        PadProbeReturn::Drop
                    }) {
                        block_probes.push(id);
                    }
                }
            }
        }

        // Inject EOS *into* each record queue sink (downstream). Element-level
        // send_event(EOS) on a filter goes to its sink pads (upstream) and
        // never reaches the muxer — leaving a moov-less "corrupt" file.
        for el in &branch.elements {
            let name = el.name();
            if name.starts_with(&queue_prefix) {
                if let Some(sink) = el.static_pad("sink") {
                    let _ = sink.send_event(gstreamer::event::Eos::new());
                }
            }
        }

        Ok(Some(PendingRecordDetach {
            channel_id,
            tag,
            pipeline,
            branch,
            rx,
            eos_probe,
            block_probes,
        }))
    }

    /// Unlink and release the branch's request pads: the video pad on
    /// `branch.video_tee` (`e`, `raw`, or legacy `t`) and every AAC/PCM pad on audio tee `a`.
    fn unlink_branch(pipeline: &gstreamer::Pipeline, branch: &Branch) {
        if let Some(tee_pad) = branch.tee_pad.as_ref() {
            if let Some(qpad) = branch.elements.first().and_then(|e| e.static_pad("sink")) {
                let _ = tee_pad.unlink(&qpad);
            }
            match pipeline.by_name(branch.video_tee) {
                Some(video_tee) => video_tee.release_request_pad(tee_pad),
                None => tracing::warn!(
                    tee = branch.video_tee,
                    role = branch.role.tag(),
                    "video tee missing while releasing record pad"
                ),
            }
        }
        if let Some(a_tee) = pipeline.by_name("a") {
            for audio_pad in &branch.audio_tee_pads {
                if let Some(peer) = audio_pad.peer() {
                    let _ = audio_pad.unlink(&peer);
                }
                a_tee.release_request_pad(audio_pad);
            }
        }
    }

    /// DeckLink `signal` property: true when a valid input is present.
    /// `None` if the source has no such property (non-DeckLink graph).
    fn decklink_signal_present(&self) -> Option<bool> {
        let dl = self.pipeline.as_ref()?.by_name("dlsrc")?;
        if dl.find_property("signal").is_none() {
            return None;
        }
        Some(dl.property::<bool>("signal"))
    }

    fn apply_signal_status(&mut self, playing: bool) {
        if !playing || self.status == ChannelStatus::Error {
            return;
        }
        match self.decklink_signal_present() {
            Some(false) => {
                // Drop stale peaks immediately — DeckLink can stop posting `level`
                // while the last dBFS values would otherwise stick in the UI.
                if self.status != ChannelStatus::Waiting {
                    self.audio_peaks = [-90.0; 8];
                    self.last_level_at = None;
                }
                self.status = ChannelStatus::Waiting;
            }
            Some(true) => {
                // Only clear flip budget after a real signal lock, not during relaunch
                // where `signal` can flicker and reset the counter forever.
                let relaunching = self
                    .last_adapt
                    .map(|t| t.elapsed() < std::time::Duration::from_secs(4))
                    .unwrap_or(false);
                if !relaunching {
                    self.signal_flip_attempts = 0;
                }
                self.status = ChannelStatus::Running;
            }
            None => {
                // Unknown source — keep previous status if already decided.
                if self.status != ChannelStatus::Error {
                    self.status = ChannelStatus::Running;
                }
            }
        }
    }

    /// If `level` goes quiet (no posts), decay meters so silence / stalled audio
    /// never shows a frozen peak from an earlier route.
    fn decay_stale_peaks(&mut self) {
        if self.status != ChannelStatus::Running {
            return;
        }
        let stale = self
            .last_level_at
            .map(|t| t.elapsed() > std::time::Duration::from_millis(150))
            .unwrap_or(false);
        if stale {
            self.audio_peaks = [-90.0; 8];
        }
    }

    /// Peaks / status / bus drain only — no format adapt or graph relaunch.
    /// Used by the high-frequency meter poll so UI meters never hold `gst_op`
    /// across a capture relaunch.
    pub fn poll_bus_light(&mut self) {
        let playing = {
            let Some(p) = self.pipeline.as_ref() else {
                return;
            };
            let (_, cur, _) = p.state(gstreamer::ClockTime::ZERO);
            cur == gstreamer::State::Playing
        };
        self.apply_signal_status(playing);
        self.decay_stale_peaks();
        if let Some(fmt) = self.read_live_format() {
            let changed = self
                .detected
                .as_ref()
                .map(|d| d.mode != fmt.mode)
                .unwrap_or(true);
            self.detected = Some(fmt.clone());
            if changed {
                tracing::info!(
                    channel = self.id,
                    format = %fmt.summary(),
                    "input format"
                );
            }
        }
        self.drain_bus_messages();
    }

    pub fn poll_bus(&mut self) {
        let playing = {
            let Some(p) = self.pipeline.as_ref() else {
                return;
            };
            let (_, cur, _) = p.state(gstreamer::ClockTime::ZERO);
            cur == gstreamer::State::Playing
        };
        self.apply_signal_status(playing);
        self.decay_stale_peaks();

        // Auto channels locked to the wrong scan (p50 vs i50) often sit in Waiting with
        // signal=false. Flip once after a long wait so a mis-probe can recover — then stop.
        let flip_to = if playing
            && is_auto_mode(&self.configured_mode)
            && self.status == ChannelStatus::Waiting
            && self.decklink_signal_present() == Some(false)
            && self.signal_flip_attempts < 1
            && self
                .last_signal_flip
                .map(|t| t.elapsed() > std::time::Duration::from_secs(12))
                .unwrap_or(true)
            && self
                .last_adapt
                .map(|t| t.elapsed() > std::time::Duration::from_secs(12))
                .unwrap_or(true)
        {
            match self.locked_mode.as_str() {
                "1080p50" => Some("1080i50"),
                "1080i50" => Some("1080p50"),
                "1080p5994" => Some("1080i5994"),
                "1080i5994" => Some("1080p5994"),
                "1080p60" => Some("1080i60"),
                "1080i60" => Some("1080p60"),
                _ => None,
            }
        } else {
            None
        };
        if let Some(mode) = flip_to {
            tracing::info!(
                channel = self.id,
                from = %self.locked_mode,
                to = %mode,
                attempt = self.signal_flip_attempts + 1,
                "no signal — trying alternate scan mode"
            );
            self.last_signal_flip = Some(std::time::Instant::now());
            self.signal_flip_attempts = self.signal_flip_attempts.saturating_add(1);
            self.pending_adapt_mode = None;
            let _ = self.adapt_to_mode(mode);
            return;
        }

        let adapt_to = {
            if let Some(fmt) = self.read_live_format() {
                let changed = self
                    .detected
                    .as_ref()
                    .map(|d| d.mode != fmt.mode)
                    .unwrap_or(true);
                self.detected = Some(fmt.clone());
                if !is_auto_mode(&self.configured_mode)
                    || fmt.mode == self.locked_mode
                    || fmt.width < 1280
                {
                    self.pending_adapt_mode = None;
                    if changed {
                        tracing::info!(
                            channel = self.id,
                            format = %fmt.summary(),
                            "input format"
                        );
                    }
                    None
                } else {
                    let cool = self
                        .last_adapt
                        .map(|t| t.elapsed() > std::time::Duration::from_secs(10))
                        .unwrap_or(true);
                    let same_pending = self
                        .pending_adapt_mode
                        .as_ref()
                        .is_some_and(|(m, _)| m == &fmt.mode);
                    let stable = if same_pending {
                        self.pending_adapt_mode
                            .as_ref()
                            .is_some_and(|(_, since)| {
                                since.elapsed() > std::time::Duration::from_secs(2)
                            })
                    } else {
                        self.pending_adapt_mode =
                            Some((fmt.mode.clone(), std::time::Instant::now()));
                        false
                    };
                    if cool && stable {
                        self.pending_adapt_mode = None;
                        Some(fmt.mode)
                    } else {
                        if changed {
                            tracing::info!(
                                channel = self.id,
                                format = %fmt.summary(),
                                "input format (waiting for stability before adapt)"
                            );
                        }
                        None
                    }
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

        self.drain_bus_messages();
    }

    fn drain_bus_messages(&mut self) {
        let Some(p) = self.pipeline.as_ref() else {
            return;
        };
        let bus = p.bus().expect("pipeline bus");
        let mut lost_signal_warn = false;
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
                MessageView::StateChanged(_) => {
                    // Running vs Waiting is decided from DeckLink `signal` at poll start.
                }
                MessageView::Warning(w) => {
                    let text = format!("{} ({})", w.error(), w.debug().unwrap_or_default());
                    let lower = text.to_ascii_lowercase();
                    // Expected noise: unused DeckLink inputs, brief backlog after attach/restart.
                    if lower.contains("signal lost") || lower.contains("no input source") {
                        lost_signal_warn = true;
                        tracing::debug!(channel = self.id, %text, "gst warning (no signal)");
                    } else if lower.contains("dropped") && lower.contains("old frames") {
                        tracing::debug!(channel = self.id, %text, "gst warning (benign)");
                    } else {
                        tracing::warn!(channel = self.id, %text, "gst warning");
                    }
                }
                MessageView::Element(el) => {
                    if let Some(s) = el.structure() {
                        if s.name() == "level" {
                            Self::apply_level_structure(&mut self.audio_peaks, s);
                            self.last_level_at = Some(std::time::Instant::now());
                        }
                    }
                }
                _ => {}
            }
        }
        if lost_signal_warn && self.status != ChannelStatus::Error {
            self.status = ChannelStatus::Waiting;
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
        // Relaunch hard-cuts the graph. Finalizing REC under the global gst_op lock
        // would stall every channel; skipping adapt keeps the muxer footer intact.
        if self.proxy_recording || self.hq_recording {
            tracing::warn!(
                channel = self.id,
                from = %self.locked_mode,
                to = %new_mode,
                "skipping format adapt while recording"
            );
            return Ok(());
        }
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
            record_preset: self.record_preset_id.clone(),
            video_bitrate_kbps: self.encode_bitrate.kbps(),
            srt_bitrate_kbps: self.srt_bitrate.as_ref().and_then(|m| m.kbps()),
            recording: self.proxy_recording || self.hq_recording,
            proxy_recording: self.proxy_recording,
            hq_recording: self.hq_recording,
            srt: self.srt,
            // Backward compat: HQ path wins, otherwise fall back to the proxy path.
            recording_path: self
                .hq_recording_path
                .clone()
                .or_else(|| self.proxy_recording_path.clone()),
            proxy_recording_path: self.proxy_recording_path.clone(),
            hq_recording_path: self.hq_recording_path.clone(),
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
            bit_depth: self.detected.as_ref().map(|f| f.bit_depth.max(8)),
            mezz_label: self.mezz_label_for_signal(),
            audio_peaks: Some(self.audio_peaks.to_vec()),
            preview_epoch: self.preview_epoch,
        }
    }
}
