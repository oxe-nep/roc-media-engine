//! On-demand WebRTC encode preview (sendonly) via `webrtcbin`.
//!
//! Video is taken from the **proxy encode tee `e`** (NVENC/x264 already running).
//! Audio is taken from a **standing valve tap** on the AAC pair tee (`wpv_avN`),
//! so preview never request/releases pads on raw tee `a` or the AAC tees —
//! releasing those pads was flushing upstream and killing card `ameter`.

use anyhow::{anyhow, bail, Context, Result};
use gstreamer::prelude::*;
use gstreamer::{
    Bin, Element, GhostPad, Pad, PadProbeReturn, PadProbeType, Pipeline, Promise, State,
};
use gstreamer_sdp::SDPMessage;
use gstreamer_webrtc::{WebRTCSDPType, WebRTCSessionDescription};
use std::sync::mpsc;
use tracing::info;

use crate::preview_sig::{PreviewSignal, PreviewSignalTx};

pub struct WebRtcPreview {
    pub session_id: String,
    pub pair: u8,
    pub webrtc: Element,
    pub tee_pad: Pad,
    pub audio_ghost: Pad,
    pub branch: Bin,
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
        let video_tee = pipeline
            .by_name("e")
            .ok_or_else(|| anyhow!("encoded tee `e` missing"))?;
        // Standing AAC listen valve must exist (built with MPEG-TS program AAC).
        if pipeline.by_name(&format!("wpv_av{pair}")).is_none() {
            bail!("wpv_av{pair} missing — is MPEG-TS egress / AAC encode enabled?");
        }

        let session_id = uuid::Uuid::new_v4().to_string();
        let tag = format!("wpv{channel}p{pair}");

        let desc = format!(
            "queue name=q_v_{tag} max-size-buffers=4 leaky=downstream ! \
             h264parse config-interval=-1 ! \
             rtph264pay name=vpay_{tag} pt=96 config-interval=-1 aggregate-mode=zero-latency ! \
             application/x-rtp,media=video,encoding-name=H264,payload=96,clock-rate=90000 ! \
             webrtcbin name=webrtc_{tag} bundle-policy=max-bundle \
             queue name=q_a_{tag} max-size-buffers=4 leaky=downstream ! \
             aacparse ! avdec_aac ! audioconvert ! audioresample ! \
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
        if let Err(e) = tee_pad
            .link(&ghost_v)
            .context("link encode tee → webrtc branch (proxy must be H264)")
        {
            let _ = branch.set_state(State::Null);
            let _ = pipeline.remove(&branch);
            video_tee.release_request_pad(&tee_pad);
            return Err(e);
        }

        let audio_ghost: Pad = ghost_a.upcast();
        if let Err(e) = wire_audio_valve(pipeline, pair, &audio_ghost) {
            cut_tee_pad(pipeline, "e", &tee_pad);
            let _ = branch.set_state(State::Null);
            let _ = pipeline.remove(&branch);
            return Err(e);
        }

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

        if let Err(e) = branch.sync_state_with_parent().context("sync webrtc branch") {
            let _ = restore_audio_valve(pipeline, pair, &audio_ghost);
            cut_tee_pad(pipeline, "e", &tee_pad);
            let _ = branch.set_state(State::Null);
            let _ = pipeline.remove(&branch);
            return Err(e);
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

        let (offer_tx, offer_rx) = mpsc::channel::<Result<String>>();
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
                let _ = restore_audio_valve(pipeline, pair, &audio_ghost);
                cut_tee_pad(pipeline, "e", &tee_pad);
                let _ = branch.set_state(State::Null);
                let _ = pipeline.remove(&branch);
                return Err(e);
            }
        };
        if !sdp.contains("m=video") || !sdp.contains("m=audio") {
            let _ = restore_audio_valve(pipeline, pair, &audio_ghost);
            cut_tee_pad(pipeline, "e", &tee_pad);
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
            "attached webrtc preview (proxy encode + AAC valve)"
        );
        Ok(Self {
            session_id,
            pair,
            webrtc,
            tee_pad,
            audio_ghost,
            branch,
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
        // Restore AAC valve→fakesink first (no tee pad release on audio path).
        if let Err(e) = restore_audio_valve(pipeline, self.pair, &self.audio_ghost) {
            tracing::warn!(error = %e, pair = self.pair, "failed to restore webrtc AAC valve");
        }
        cut_tee_pad(pipeline, "e", &self.tee_pad);
        let _ = self.branch.set_state(State::Null);
        let _ = pipeline.remove(&self.branch);
        info!(session = %self.session_id, "detached webrtc preview");
    }
}

fn wire_audio_valve(pipeline: &Pipeline, pair: u8, ghost_a: &Pad) -> Result<()> {
    let valve = pipeline
        .by_name(&format!("wpv_av{pair}"))
        .ok_or_else(|| anyhow!("wpv_av{pair} missing"))?;
    let fakesink = pipeline
        .by_name(&format!("wpv_as{pair}"))
        .ok_or_else(|| anyhow!("wpv_as{pair} missing"))?;
    valve.set_property("drop", true);
    let src = valve
        .static_pad("src")
        .ok_or_else(|| anyhow!("wpv_av{pair} src"))?;
    if let Some(peer) = src.peer() {
        let _ = src.unlink(&peer);
    }
    let _ = fakesink.set_state(State::Ready);
    src.link(ghost_a)
        .with_context(|| format!("link wpv_av{pair} → webrtc audio"))?;
    valve.set_property("drop", false);
    Ok(())
}

fn restore_audio_valve(pipeline: &Pipeline, pair: u8, ghost_a: &Pad) -> Result<()> {
    let valve = pipeline
        .by_name(&format!("wpv_av{pair}"))
        .ok_or_else(|| anyhow!("wpv_av{pair} missing"))?;
    let fakesink = pipeline
        .by_name(&format!("wpv_as{pair}"))
        .ok_or_else(|| anyhow!("wpv_as{pair} missing"))?;
    valve.set_property("drop", true);
    let src = valve
        .static_pad("src")
        .ok_or_else(|| anyhow!("wpv_av{pair} src"))?;
    if let Some(peer) = src.peer() {
        let parent = peer
            .parent_element()
            .map(|e| e.name().to_string())
            .unwrap_or_default();
        if parent == format!("wpv_as{pair}") {
            let _ = fakesink.sync_state_with_parent();
            return Ok(());
        }
        let _ = src.unlink(&peer);
        let _ = ghost_a; // ghost may already be unlinked with peer
    }
    let fs_sink = fakesink
        .static_pad("sink")
        .ok_or_else(|| anyhow!("wpv_as{pair} sink"))?;
    if src.peer().is_none() {
        src.link(&fs_sink)
            .with_context(|| format!("relink wpv_av{pair} → wpv_as{pair}"))?;
    }
    let _ = fakesink.sync_state_with_parent();
    Ok(())
}

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
