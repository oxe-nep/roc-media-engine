//! On-demand WebRTC encode preview (sendonly) via `webrtcbin`.
//!
//! Video is taken from the **proxy encode tee `e`** (NVENC/x264 already running)
//! so preview matches live/proxy quality without a second encode.
//! Audio is mixed from the standing `wpv_avalve` tap on tee `a` (never request/release
//! audio tee pads — that was killing card meters after preview close).
//!
//! The branch stays in a nested [`Bin`] so `parse::launch` links remain intact.

use anyhow::{anyhow, bail, Context, Result};
use gstreamer::prelude::*;
use gstreamer::{
    Bin, Element, GhostPad, Pad, PadProbeReturn, PadProbeType, Pipeline, Promise, State,
};
use gstreamer_sdp::SDPMessage;
use gstreamer_webrtc::{WebRTCSDPType, WebRTCSessionDescription};
use std::sync::mpsc;
use tracing::info;

use crate::describe::stereo_pair_matrix;
use crate::preview_sig::{PreviewSignal, PreviewSignalTx};

pub struct WebRtcPreview {
    pub session_id: String,
    pub pair: u8,
    pub webrtc: Element,
    pub tee_pad: Pad,
    /// Ghost sink on the preview bin that receives audio from `wpv_avalve`.
    pub audio_ghost: Pad,
    pub branch: Bin,
    /// True when audio was taken from the standing valve tap (preferred path).
    audio_via_valve: bool,
    /// Legacy fallback: dynamic tee pad when standby branch is missing.
    audio_tee_pad: Option<Pad>,
}

impl WebRtcPreview {
    pub fn attach(
        pipeline: &Pipeline,
        channel: u32,
        pair: u8,
        signal_tx: PreviewSignalTx,
    ) -> Result<Self> {
        if pair > 3 {
            bail!("listen pair must be 0..=3");
        }
        // Full-quality video from the live proxy encode (same bitstream as SRT/UDP).
        let video_tee = pipeline
            .by_name("e")
            .ok_or_else(|| anyhow!("encoded tee `e` missing"))?;
        let audio_tee = pipeline
            .by_name("a")
            .ok_or_else(|| anyhow!("audio tee `a` missing"))?;
        if audio_tee.find_property("allow-not-linked").is_some() {
            audio_tee.set_property("allow-not-linked", true);
        }

        let session_id = uuid::Uuid::new_v4().to_string();
        let tag = format!("wpv{channel}p{pair}");
        let matrix = stereo_pair_matrix(pair as usize);

        // Re-payload existing H264 (no scale/re-encode). SPS/PPS on every IDR for browsers.
        let desc = format!(
            "queue name=q_v_{tag} max-size-buffers=4 leaky=downstream ! \
             h264parse config-interval=-1 ! \
             rtph264pay name=vpay_{tag} pt=96 config-interval=-1 aggregate-mode=zero-latency ! \
             application/x-rtp,media=video,encoding-name=H264,payload=96,clock-rate=90000 ! \
             webrtcbin name=webrtc_{tag} bundle-policy=max-bundle \
             queue name=q_a_{tag} max-size-buffers=4 leaky=downstream ! \
             audioconvert mix-matrix=\"{matrix}\" ! \
             audio/x-raw,format=S16LE,rate=48000,channels=2,layout=interleaved ! \
             opusenc bitrate=128000 ! \
             rtpopuspay name=apay_{tag} pt=97 ! \
             application/x-rtp,media=audio,encoding-name=OPUS,payload=97,clock-rate=48000 ! \
             webrtc_{tag}."
        );

        let branch = gstreamer::parse::launch(&desc)
            .context("parse webrtc preview")?
            .downcast::<Bin>()
            .map_err(|_| anyhow!("webrtc preview launch was not a Bin"))?;

        let webrtc = branch
            .by_name(&format!("webrtc_{tag}"))
            .ok_or_else(|| anyhow!("webrtcbin missing after parse"))?;
        let q_v = branch
            .by_name(&format!("q_v_{tag}"))
            .ok_or_else(|| anyhow!("q_v missing"))?;
        let q_a = branch
            .by_name(&format!("q_a_{tag}"))
            .ok_or_else(|| anyhow!("q_a missing"))?;

        let qv_sink = q_v.static_pad("sink").ok_or_else(|| anyhow!("q_v sink"))?;
        let qa_sink = q_a.static_pad("sink").ok_or_else(|| anyhow!("q_a sink"))?;
        let ghost_v = GhostPad::builder_with_target(&qv_sink)
            .context("ghost video sink")?
            .name(format!("vsink_{tag}"))
            .build();
        let ghost_a = GhostPad::builder_with_target(&qa_sink)
            .context("ghost audio sink")?
            .name(format!("asink_{tag}"))
            .build();
        ghost_v.set_active(true).context("activate video ghost")?;
        ghost_a.set_active(true).context("activate audio ghost")?;
        branch.add_pad(&ghost_v).context("add video ghost")?;
        branch.add_pad(&ghost_a).context("add audio ghost")?;

        pipeline
            .add(&branch)
            .context("add webrtc branch to capture pipeline")?;

        let tee_pad = video_tee
            .request_pad_simple("src_%u")
            .ok_or_else(|| anyhow!("request encode tee pad"))?;
        tee_pad
            .link(&ghost_v)
            .context("link encode tee → webrtc branch (proxy must be H264)")?;

        let ghost_a_pad: Pad = ghost_a.upcast();
        let (audio_via_valve, audio_tee_pad) =
            match wire_audio_from_valve(pipeline, &ghost_a_pad) {
                Ok(()) => (true, None),
                Err(valve_err) => {
                    tracing::warn!(
                        channel,
                        error = %valve_err,
                        "webrtc audio valve tap missing — falling back to dynamic tee pad"
                    );
                    let audio_tee_pad = audio_tee
                        .request_pad_simple("src_%u")
                        .ok_or_else(|| anyhow!("request audio tee pad"))?;
                    audio_tee_pad
                        .link(&ghost_a_pad)
                        .context("link audio tee → webrtc branch")?;
                    (false, Some(audio_tee_pad))
                }
            };

        {
            let tx = signal_tx.clone();
            webrtc.connect("on-ice-candidate", false, move |values| {
                let mline = values.get(1).and_then(|v| v.get::<u32>().ok()).unwrap_or(0);
                let cand = values
                    .get(2)
                    .and_then(|v| v.get::<String>().ok())
                    .unwrap_or_default();
                if cand.is_empty() {
                    return None;
                }
                if let Some(tx) = tx.lock().as_ref() {
                    let _ = tx.send(PreviewSignal::Ice {
                        channel,
                        candidate: cand,
                        sdp_mline_index: mline,
                    });
                }
                None
            });
        }

        if let Err(e) = branch.sync_state_with_parent() {
            // Rewire audio back before bailing so the standing tap stays sane.
            if audio_via_valve {
                let _ = restore_audio_valve(pipeline, &ghost_a_pad);
            }
            cut_tee_pad(pipeline, "e", &tee_pad);
            if let Some(ref ap) = audio_tee_pad {
                cut_tee_pad(pipeline, "a", ap);
            }
            let _ = branch.set_state(State::Null);
            let _ = pipeline.remove(&branch);
            return Err(e).context("sync webrtc branch");
        }

        let vpay = branch
            .by_name(&format!("vpay_{tag}"))
            .ok_or_else(|| anyhow!("vpay missing"))?;
        let vpay_src = vpay
            .static_pad("src")
            .ok_or_else(|| anyhow!("vpay src missing"))?;
        let mut have_fmtp = false;
        for _ in 0..75 {
            if let Some(caps) = vpay_src.current_caps() {
                let s = caps.to_string();
                if s.contains("packetization-mode") || s.contains("profile-level-id") {
                    have_fmtp = true;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if !have_fmtp {
            tracing::debug!(channel, "H264 RTP caps still missing fmtp fields before create-offer");
        }

        // Do NOT wait on set-local-description inside this callback — that deadlocks.
        let (offer_tx, offer_rx) = std::sync::mpsc::channel::<Result<String>>();
        let promise = Promise::with_change_func({
            let webrtc = webrtc.clone();
            move |reply| {
                let res = (|| -> Result<String> {
                    let reply = reply
                        .map_err(|e| anyhow!("create-offer: {e:?}"))?
                        .ok_or_else(|| anyhow!("create-offer: empty reply"))?;
                    let offer = reply
                        .value("offer")
                        .context("offer field")?
                        .get::<WebRTCSessionDescription>()
                        .context("offer type")?;
                    webrtc.emit_by_name::<()>(
                        "set-local-description",
                        &[&offer, &None::<Promise>],
                    );
                    let mut sdp = offer.sdp().as_text().context("sdp text")?;
                    sdp = sdp.replace("a=sendrecv", "a=sendonly");
                    Ok(sdp)
                })();
                let _ = offer_tx.send(res);
            }
        });
        webrtc.emit_by_name::<()>(
            "create-offer",
            &[&None::<gstreamer::Structure>, &promise],
        );
        let sdp = match offer_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .context("create-offer timeout")
            .and_then(|r| r.context("create-offer"))
        {
            Ok(s) => s,
            Err(e) => {
                if audio_via_valve {
                    let _ = restore_audio_valve(pipeline, &ghost_a_pad);
                }
                cut_tee_pad(pipeline, "e", &tee_pad);
                if let Some(ref ap) = audio_tee_pad {
                    cut_tee_pad(pipeline, "a", ap);
                }
                let _ = branch.set_state(State::Null);
                let _ = pipeline.remove(&branch);
                return Err(e);
            }
        };
        if !sdp.contains("m=video") || !sdp.contains("m=audio") {
            if audio_via_valve {
                let _ = restore_audio_valve(pipeline, &ghost_a_pad);
            }
            cut_tee_pad(pipeline, "e", &tee_pad);
            if let Some(ref ap) = audio_tee_pad {
                cut_tee_pad(pipeline, "a", ap);
            }
            let _ = branch.set_state(State::Null);
            let _ = pipeline.remove(&branch);
            bail!("webrtc offer missing media lines (sdp_len={})", sdp.len());
        }
        if let Some(tx) = signal_tx.lock().as_ref() {
            let _ = tx.send(PreviewSignal::Offer {
                channel,
                sdp: sdp.clone(),
            });
        }

        info!(
            channel,
            pair,
            %session_id,
            audio_via_valve,
            "attached webrtc preview (proxy encode)"
        );
        Ok(Self {
            session_id,
            pair,
            webrtc,
            tee_pad,
            audio_ghost: ghost_a_pad,
            branch,
            audio_via_valve,
            audio_tee_pad,
        })
    }

    pub fn set_remote_answer(&self, sdp_text: &str) -> Result<()> {
        let sdp = SDPMessage::parse_buffer(sdp_text.as_bytes()).context("parse answer SDP")?;
        let answer = WebRTCSessionDescription::new(WebRTCSDPType::Answer, sdp);
        let (tx, rx) = mpsc::channel::<Result<()>>();
        let promise = Promise::with_change_func(move |reply| {
            let res = reply
                .map_err(|e| anyhow!("set-remote-description: {e:?}"))
                .map(|_| ());
            let _ = tx.send(res);
        });
        self.webrtc
            .emit_by_name::<()>("set-remote-description", &[&answer, &promise]);
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .context("set-remote-description timeout")?
            .context("set-remote-description")?;
        info!(session = %self.session_id, "webrtc remote answer applied");
        Ok(())
    }

    pub fn add_ice_candidate(&self, sdp_mline_index: u32, candidate: &str) {
        self.webrtc
            .emit_by_name::<()>("add-ice-candidate", &[&sdp_mline_index, &candidate]);
    }

    pub fn detach(self, pipeline: &Pipeline) {
        // Audio first: restore standing valve→fakesink without releasing tee pads.
        if self.audio_via_valve {
            if let Err(e) = restore_audio_valve(pipeline, &self.audio_ghost) {
                tracing::warn!(error = %e, "failed to restore webrtc audio valve tap");
            }
        } else if let Some(ref ap) = self.audio_tee_pad {
            cut_tee_pad(pipeline, "a", ap);
        }
        cut_tee_pad(pipeline, "e", &self.tee_pad);
        let _ = self.branch.set_state(State::Null);
        let _ = pipeline.remove(&self.branch);
        rebuild_audio_meter(pipeline);
        info!(session = %self.session_id, "detached webrtc preview");
    }
}

/// Point standing `wpv_avalve` at the preview ghost (drop=false while live).
fn wire_audio_from_valve(pipeline: &Pipeline, ghost_a: &Pad) -> Result<()> {
    let valve = pipeline
        .by_name("wpv_avalve")
        .ok_or_else(|| anyhow!("wpv_avalve missing"))?;
    let fakesink = pipeline
        .by_name("wpv_asink")
        .ok_or_else(|| anyhow!("wpv_asink missing"))?;
    valve.set_property("drop", true);
    let src = valve
        .static_pad("src")
        .ok_or_else(|| anyhow!("wpv_avalve src"))?;
    if let Some(peer) = src.peer() {
        let _ = src.unlink(&peer);
    }
    // Ensure fakesink is idle while preview owns the valve output.
    let _ = fakesink.set_state(State::Ready);
    src.link(ghost_a).context("link wpv_avalve → webrtc audio")?;
    valve.set_property("drop", false);
    Ok(())
}

/// Restore `wpv_avalve` → `wpv_asink` and keep dropping until next preview.
fn restore_audio_valve(pipeline: &Pipeline, ghost_a: &Pad) -> Result<()> {
    let valve = pipeline
        .by_name("wpv_avalve")
        .ok_or_else(|| anyhow!("wpv_avalve missing"))?;
    let fakesink = pipeline
        .by_name("wpv_asink")
        .ok_or_else(|| anyhow!("wpv_asink missing"))?;
    valve.set_property("drop", true);
    let src = valve
        .static_pad("src")
        .ok_or_else(|| anyhow!("wpv_avalve src"))?;
    if let Some(peer) = src.peer() {
        let peer_name = peer.name().to_string();
        let parent_name = peer
            .parent_element()
            .map(|e| e.name().to_string())
            .unwrap_or_default();
        if parent_name == "wpv_asink" {
            let _ = fakesink.sync_state_with_parent();
            return Ok(());
        }
        if peer_name == ghost_a.name().as_str() || peer_name.contains("asink_") {
            let _ = src.unlink(&peer);
        } else {
            let _ = src.unlink(&peer);
        }
    }
    let fs_sink = fakesink
        .static_pad("sink")
        .ok_or_else(|| anyhow!("wpv_asink sink"))?;
    if src.peer().is_none() {
        src.link(&fs_sink)
            .context("relink wpv_avalve → wpv_asink")?;
    }
    let _ = fakesink.sync_state_with_parent();
    Ok(())
}

/// Unlink + release a tee request pad when the pad is idle (safe vs racing buffers).
fn cut_tee_pad(pipeline: &Pipeline, tee_name: &str, tee_pad: &Pad) {
    let tee = pipeline.by_name(tee_name);
    let (tx, rx) = mpsc::channel::<()>();
    let tee_for_probe = tee.clone();
    let probe_id = tee_pad.add_probe(PadProbeType::IDLE, move |pad, _| {
        if let Some(peer) = pad.peer() {
            let _ = pad.unlink(&peer);
        }
        if let Some(ref t) = tee_for_probe {
            t.release_request_pad(pad);
        }
        let _ = tx.send(());
        PadProbeReturn::Remove
    });

    if rx
        .recv_timeout(std::time::Duration::from_millis(750))
        .is_err()
    {
        if let Some(id) = probe_id {
            tee_pad.remove_probe(id);
        }
        if let Some(peer) = tee_pad.peer() {
            let _ = tee_pad.unlink(&peer);
        }
        if let Some(t) = tee {
            t.release_request_pad(tee_pad);
        }
    }
}

/// Tear down and recreate the card meter branch if a prior flush wedged it.
fn rebuild_audio_meter(pipeline: &Pipeline) {
    let Some(tee) = pipeline.by_name("a") else {
        return;
    };

    // Drop existing meter elements (and their tee request pad).
    if let Some(q) = pipeline.by_name("ameter_q") {
        if let Some(sink) = q.static_pad("sink") {
            if let Some(peer) = sink.peer() {
                let _ = peer.unlink(&sink);
                if peer.parent_element().as_ref().map(|e| e.name().as_str()) == Some("a") {
                    tee.release_request_pad(&peer);
                }
            }
        }
    }
    for name in ["ameter_sink", "ameter", "ameter_q"] {
        if let Some(el) = pipeline.by_name(name) {
            let _ = el.set_state(State::Null);
            let _ = pipeline.remove(&el);
        }
    }

    let Ok(q) = gstreamer::ElementFactory::make("queue")
        .name("ameter_q")
        .property("max-size-buffers", 8u32)
        .property_from_str("leaky", "downstream")
        .build()
    else {
        tracing::warn!("rebuild ameter: queue factory failed");
        return;
    };
    let Ok(level) = gstreamer::ElementFactory::make("level")
        .name("ameter")
        .property("interval", 33_000_000u64)
        .property("post-messages", true)
        .build()
    else {
        tracing::warn!("rebuild ameter: level factory failed");
        return;
    };
    let Ok(sink) = gstreamer::ElementFactory::make("fakesink")
        .name("ameter_sink")
        .property("sync", false)
        .property("async", false)
        .build()
    else {
        tracing::warn!("rebuild ameter: fakesink factory failed");
        return;
    };

    if pipeline.add_many([&q, &level, &sink]).is_err() {
        tracing::warn!("rebuild ameter: add_many failed");
        return;
    }
    if q.link(&level).is_err() || level.link(&sink).is_err() {
        tracing::warn!("rebuild ameter: link failed");
        return;
    }
    let Some(tee_pad) = tee.request_pad_simple("src_%u") else {
        tracing::warn!("rebuild ameter: tee pad request failed");
        return;
    };
    let Some(q_sink) = q.static_pad("sink") else {
        return;
    };
    if tee_pad.link(&q_sink).is_err() {
        tracing::warn!("rebuild ameter: tee link failed");
        tee.release_request_pad(&tee_pad);
        return;
    }
    for el in [&q, &level, &sink] {
        if let Err(e) = el.sync_state_with_parent() {
            tracing::warn!(error = %e, "rebuild ameter: sync failed");
        }
    }
    info!("rebuilt audio meter branch after webrtc detach");
}
