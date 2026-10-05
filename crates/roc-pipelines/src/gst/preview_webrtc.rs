//! On-demand WebRTC encode preview (sendonly) via `webrtcbin`.
//!
//! Hot-attaches to raw tees `t` / `a` for one stereo pair at a time.
//! Signaling (offer / answer / ICE) is owned by the UI WebSocket layer.
//!
//! The encode branch stays in a nested [`Bin`] so `parse::launch` links
//! (payloader → webrtcbin) remain intact — moving children out of a parse
//! bin unlinks pads and yields an empty SDP offer.

use anyhow::{anyhow, bail, Context, Result};
use gstreamer::prelude::*;
use gstreamer::{Bin, Element, GhostPad, Pad, Pipeline, Promise, State};
use gstreamer_sdp::SDPMessage;
use gstreamer_webrtc::{WebRTCSDPType, WebRTCSessionDescription};
use tracing::{info, warn};

use crate::describe::stereo_pair_matrix;
use crate::preview_sig::{PreviewSignal, PreviewSignalTx};

pub struct WebRtcPreview {
    pub session_id: String,
    pub pair: u8,
    pub webrtc: Element,
    pub tee_pad: Pad,
    pub audio_tee_pad: Pad,
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
            .by_name("t")
            .ok_or_else(|| anyhow!("raw tee `t` missing"))?;
        let audio_tee = pipeline
            .by_name("a")
            .ok_or_else(|| anyhow!("audio tee `a` missing"))?;

        let session_id = uuid::Uuid::new_v4().to_string();
        let tag = format!("wpv{channel}p{pair}");
        let matrix = stereo_pair_matrix(pair as usize);

        // Browser-friendly H264: constrained-baseline, SPS/PPS on every IDR,
        // packetization-mode=1 via rtph264pay aggregate-mode=zero-latency.
        let desc = format!(
            "queue name=q_v_{tag} max-size-buffers=2 leaky=downstream ! \
             videoconvert ! videoscale ! videorate skip-to-first=true ! \
             video/x-raw,width=640,height=360,framerate=15/1 ! \
             x264enc name=venc_{tag} tune=zerolatency speed-preset=ultrafast bitrate=800 \
             key-int-max=15 bframes=0 threads=1 byte-stream=true \
             option-string=\"repeat-headers=1\" ! \
             video/x-h264,profile=constrained-baseline,stream-format=byte-stream,alignment=au ! \
             h264parse config-interval=-1 ! \
             rtph264pay name=vpay_{tag} pt=96 config-interval=-1 aggregate-mode=zero-latency ! \
             application/x-rtp,media=video,encoding-name=H264,payload=96,clock-rate=90000 ! \
             webrtcbin name=webrtc_{tag} stun-server=stun://stun.l.google.com:19302 \
             bundle-policy=max-bundle \
             queue name=q_a_{tag} max-size-buffers=4 leaky=downstream ! \
             audioconvert mix-matrix=\"{matrix}\" ! \
             audio/x-raw,format=S16LE,rate=48000,channels=2,layout=interleaved ! \
             opusenc bitrate=64000 ! \
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
            .ok_or_else(|| anyhow!("request video tee pad"))?;
        tee_pad
            .link(&ghost_v)
            .context("link video tee → webrtc branch")?;

        let audio_tee_pad = audio_tee
            .request_pad_simple("src_%u")
            .ok_or_else(|| anyhow!("request audio tee pad"))?;
        audio_tee_pad
            .link(&ghost_a)
            .context("link audio tee → webrtc branch")?;

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

        branch
            .sync_state_with_parent()
            .context("sync webrtc branch")?;

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
                    let (local_tx, local_rx) = std::sync::mpsc::channel::<Result<()>>();
                    let local_promise = Promise::with_change_func(move |r| {
                        let _ = local_tx.send(
                            r.map_err(|e| anyhow!("set-local-description: {e:?}"))
                                .map(|_| ()),
                        );
                    });
                    webrtc.emit_by_name::<()>("set-local-description", &[&offer, &local_promise]);
                    local_rx
                        .recv_timeout(std::time::Duration::from_secs(3))
                        .context("set-local-description timeout")??;
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
        let sdp = offer_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .context("create-offer timeout")?
            .context("create-offer")?;
        if !sdp.contains("m=video") || !sdp.contains("m=audio") {
            bail!("webrtc offer missing media lines (sdp_len={})", sdp.len());
        }
        if !sdp.to_ascii_lowercase().contains("packetization-mode") {
            warn!(channel, "offer H264 fmtp may lack packetization-mode");
        }
        if let Some(tx) = signal_tx.lock().as_ref() {
            let _ = tx.send(PreviewSignal::Offer {
                channel,
                sdp: sdp.clone(),
            });
        }

        info!(channel, pair, %session_id, "attached webrtc preview");
        Ok(Self {
            session_id,
            pair,
            webrtc,
            tee_pad,
            audio_tee_pad,
            branch,
        })
    }

    pub fn set_remote_answer(&self, sdp_text: &str) -> Result<()> {
        let sdp = SDPMessage::parse_buffer(sdp_text.as_bytes()).context("parse answer SDP")?;
        let answer = WebRTCSessionDescription::new(WebRTCSDPType::Answer, sdp);
        let (tx, rx) = std::sync::mpsc::channel::<Result<()>>();
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
        let video_tee = pipeline.by_name("t");
        let audio_tee = pipeline.by_name("a");
        if let Some(peer) = self.tee_pad.peer() {
            let _ = self.tee_pad.unlink(&peer);
        }
        if let Some(t) = video_tee {
            t.release_request_pad(&self.tee_pad);
        }
        if let Some(peer) = self.audio_tee_pad.peer() {
            let _ = self.audio_tee_pad.unlink(&peer);
        }
        if let Some(t) = audio_tee {
            t.release_request_pad(&self.audio_tee_pad);
        }
        let _ = self.branch.set_state(State::Null);
        let _ = pipeline.remove(&self.branch);
        info!(session = %self.session_id, "detached webrtc preview");
    }
}
