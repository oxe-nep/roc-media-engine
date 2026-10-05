"use client";

import { useEffect, useRef, useState } from "react";
import AudioMeters from "@/components/AudioMeters";
import { LISTEN_PAIRS } from "@/components/ListenButton";
import { useBodyScrollLock } from "@/hooks/useBodyScrollLock";
import { mediaBase } from "@/lib/mediaBase";

type Props = {
  channelId: number;
  channelName: string;
  open: boolean;
  initialPair?: number;
  onClose: () => void;
};

const API_KEY = process.env.NEXT_PUBLIC_API_KEY ?? "";

function previewWsURL(): string {
  const u = new URL("/ws", mediaBase());
  u.protocol = u.protocol === "https:" ? "wss:" : "ws:";
  u.search = "";
  if (API_KEY) u.searchParams.set("api_key", API_KEY);
  return u.toString();
}

/**
 * Full-size encode preview: JPEG grid opens this modal; WebRTC carries A/V.
 * Signaling rides the same dashboard WebSocket protocol (`preview_*` messages).
 */
export default function PreviewModal({
  channelId,
  channelName,
  open,
  initialPair = 0,
  onClose,
}: Props) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const pcRef = useRef<RTCPeerConnection | null>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  const [pair, setPair] = useState(initialPair);
  const [session, setSession] = useState(0);
  const [status, setStatus] = useState("Connecting…");
  const [error, setError] = useState<string | null>(null);

  useBodyScrollLock(open);

  useEffect(() => {
    if (open) setPair(initialPair);
  }, [open, initialPair]);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    let remoteReady = false;
    const pendingIce: RTCIceCandidateInit[] = [];
    setError(null);
    setStatus("Connecting…");

    const pc = new RTCPeerConnection({
      iceServers: [{ urls: "stun:stun.l.google.com:19302" }],
    });
    pcRef.current = pc;
    pc.addTransceiver("video", { direction: "recvonly" });
    pc.addTransceiver("audio", { direction: "recvonly" });

    const attachStream = (stream: MediaStream) => {
      const video = videoRef.current;
      if (!video) return;
      video.srcObject = stream;
      void video.play().catch(() => {});
    };

    pc.ontrack = (ev) => {
      if (cancelled) return;
      if (ev.streams[0]) {
        attachStream(ev.streams[0]);
      } else {
        const stream =
          videoRef.current?.srcObject instanceof MediaStream
            ? videoRef.current.srcObject
            : new MediaStream();
        stream.addTrack(ev.track);
        attachStream(stream);
      }
      if (ev.track.kind === "video") setStatus("Live");
      else if (pc.connectionState === "connected") setStatus("Live");
    };

    pc.onconnectionstatechange = () => {
      if (cancelled) return;
      const st = pc.connectionState;
      if (st === "connected") setStatus("Live");
      else if (st === "failed") {
        setError("WebRTC connection failed");
        setStatus("Error");
      } else if (st === "connecting") setStatus("Connecting media…");
      else if (st === "disconnected") setStatus("Disconnected");
    };

    pc.oniceconnectionstatechange = () => {
      if (cancelled) return;
      const st = pc.iceConnectionState;
      if (st === "checking") setStatus("ICE…");
      else if (st === "connected" || st === "completed") {
        if (pc.connectionState !== "connected") setStatus("DTLS…");
      } else if (st === "failed") {
        setError("ICE failed");
        setStatus("Error");
      }
    };

    const ws = new WebSocket(previewWsURL());
    wsRef.current = ws;

    const send = (msg: Record<string, unknown>) => {
      if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(msg));
    };

    const flushIce = async () => {
      while (pendingIce.length) {
        const c = pendingIce.shift();
        if (!c) break;
        try {
          await pc.addIceCandidate(c);
        } catch {
          /* ignore late ICE */
        }
      }
    };

    pc.onicecandidate = (ev) => {
      if (!ev.candidate) return;
      send({
        type: "preview_ice",
        channel: channelId,
        candidate: ev.candidate.candidate,
        sdpMLineIndex: ev.candidate.sdpMLineIndex ?? 0,
      });
    };

    ws.onopen = () => {
      if (cancelled) return;
      send({ type: "preview_open", channel: channelId, pair });
      setStatus("Negotiating…");
    };

    ws.onmessage = (ev) => {
      if (cancelled) return;
      let msg: {
        type?: string;
        channel?: number;
        sdp?: string;
        candidate?: string;
        sdpMLineIndex?: number;
        message?: string;
      };
      try {
        msg = JSON.parse(String(ev.data));
      } catch {
        return;
      }
      if (msg.type === "preview_offer" && msg.sdp) {
        void (async () => {
          try {
            await pc.setRemoteDescription({ type: "offer", sdp: msg.sdp });
            remoteReady = true;
            await flushIce();
            const answer = await pc.createAnswer();
            await pc.setLocalDescription(answer);
            send({ type: "preview_answer", channel: channelId, sdp: answer.sdp });
            setStatus("ICE…");
          } catch (e) {
            setError(String(e));
            setStatus("Error");
          }
        })();
      } else if (msg.type === "preview_ice" && msg.candidate) {
        const init: RTCIceCandidateInit = {
          candidate: msg.candidate,
          sdpMLineIndex: msg.sdpMLineIndex ?? 0,
        };
        if (!remoteReady) {
          pendingIce.push(init);
        } else {
          void pc.addIceCandidate(init).catch(() => {});
        }
      } else if (msg.type === "preview_error") {
        setError(msg.message || "Preview failed");
        setStatus("Error");
      }
    };

    ws.onerror = () => {
      if (!cancelled) {
        setError("WebSocket error");
        setStatus("Error");
      }
    };
    ws.onclose = () => {
      if (!cancelled) setStatus((s) => (s === "Live" || s === "Error" ? s : "Disconnected"));
    };

    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCloseRef.current();
    };
    window.addEventListener("keydown", onKey);

    return () => {
      cancelled = true;
      window.removeEventListener("keydown", onKey);
      try {
        send({ type: "preview_close", channel: channelId });
      } catch {
        /* ignore */
      }
      try {
        ws.close();
      } catch {
        /* ignore */
      }
      try {
        pc.close();
      } catch {
        /* ignore */
      }
      pcRef.current = null;
      wsRef.current = null;
      if (videoRef.current) videoRef.current.srcObject = null;
    };
    // Intentionally omit onClose — parent passes an inline lambda that changes every render.
  }, [open, channelId, pair, session]);

  if (!open) return null;

  const live = status === "Live";
  const showOverlay = Boolean(error) || status === "Error" || status === "Disconnected";

  return (
    <div
      className="modal-backdrop library-backdrop"
      onClick={() => onCloseRef.current()}
      role="presentation"
    >
      <div
        className="modal-panel preview-modal"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-label={`Preview ${channelName}`}
      >
        <div className="preview-modal-header">
          <div className="preview-modal-title">
            <h2>Preview · {channelName}</h2>
            <span className={`preview-modal-status${live ? " live" : ""}`}>{status}</span>
          </div>
          <div className="preview-modal-actions">
            <div className="preview-pair-pills" role="group" aria-label="Audio pair">
              {LISTEN_PAIRS.map((p) => (
                <button
                  key={p.id}
                  type="button"
                  className={`preview-pair-pill${pair === p.id ? " on" : ""}`}
                  onClick={() => setPair(p.id)}
                  title={`Listen ${p.label}`}
                >
                  {p.label}
                </button>
              ))}
            </div>
            <button
              type="button"
              className="modal-close"
              onClick={() => onCloseRef.current()}
              aria-label="Close"
            >
              ×
            </button>
          </div>
        </div>
        <div className="preview-modal-stage">
          <AudioMeters channelId={channelId} bus="encode">
            <div className="preview-modal-video-wrap">
              <div className="preview-modal-video-frame">
                <video ref={videoRef} className="preview-modal-video" playsInline autoPlay />
                {showOverlay && (
                  <div className="preview-modal-overlay" role="alert">
                    <p className="preview-modal-overlay-msg">
                      {error || (status === "Disconnected" ? "Disconnected" : "Preview failed")}
                    </p>
                    <button
                      type="button"
                      className="preview-modal-reconnect"
                      onClick={() => {
                        setError(null);
                        setSession((n) => n + 1);
                      }}
                    >
                      Reconnect
                    </button>
                  </div>
                )}
              </div>
            </div>
          </AudioMeters>
        </div>
      </div>
    </div>
  );
}
