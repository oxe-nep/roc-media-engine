"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import AudioMeters from "@/components/AudioMeters";
import { LISTEN_PAIRS } from "@/components/ListenButton";
import { useDashboard } from "@/hooks/useDashboard";
import { mediaBase } from "@/lib/mediaBase";

const API_KEY = process.env.NEXT_PUBLIC_API_KEY ?? "";

function previewWsURL(): string {
  const u = new URL("/ws", mediaBase());
  u.protocol = u.protocol === "https:" ? "wss:" : "ws:";
  u.search = "";
  if (API_KEY) u.searchParams.set("api_key", API_KEY);
  return u.toString();
}

export type WebRtcPreviewPlayerProps = {
  channelId: number;
  channelName: string;
  initialPair?: number;
  /** Compact chrome for pop-out window. */
  variant?: "modal" | "window";
  onClose?: () => void;
};

/**
 * Shared WebRTC encode preview stage (modal + pop-out window).
 */
export default function WebRtcPreviewPlayer({
  channelId,
  channelName,
  initialPair = 0,
  variant = "modal",
  onClose,
}: WebRtcPreviewPlayerProps) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const frameRef = useRef<HTMLDivElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  const { streams, tcById } = useDashboard();
  const stream = streams.find((s) => s.id === channelId);
  const channelStatus = stream?.status;
  const tc = tcById[channelId];
  // Exclusive TC stops encode capture but still has proxy tee `e` for WebRTC/SRT.
  const tcPreviewReady =
    tc?.status === "running" || tc?.status === "restarting" || !!tc?.enabled;
  const encodeOff =
    !tcPreviewReady && (channelStatus === "stopped" || channelStatus === "error");
  const noSignal =
    tcPreviewReady
      ? tc?.status !== "running"
      : channelStatus === "waiting";
  const meterBus = tcPreviewReady ? "playout" : "encode";

  const signalBits = (() => {
    const raw = stream?.format?.trim();
    if (!raw) return null;
    const paren = raw.match(/\(([^)]+)\)\s*$/);
    const fmt = (paren?.[1] || raw).trim();
    const depth = stream?.bit_depth;
    if (depth === 8 || depth === 10) return `${fmt} · ${depth}-bit`;
    return fmt;
  })();
  const mezzHint = stream?.mezz_label?.trim() || null;
  const mezzDownconvert8 =
    stream?.bit_depth === 10 &&
    !!mezzHint &&
    !/x\b/i.test(mezzHint);
  const [pair, setPair] = useState(initialPair);
  const [session, setSession] = useState(0);
  const [status, setStatus] = useState("Connecting…");
  const [error, setError] = useState<string | null>(null);
  const [needsUnmute, setNeedsUnmute] = useState(false);

  useEffect(() => {
    setPair(initialPair);
  }, [initialPair]);

  const reconnect = useCallback(() => {
    setError(null);
    setSession((n) => n + 1);
  }, []);

  const tryAutostart = useCallback(async (preferUnmuted: boolean) => {
    const video = videoRef.current;
    if (!video) return;
    try {
      if (preferUnmuted) {
        video.muted = false;
        await video.play();
        setNeedsUnmute(false);
        return;
      }
    } catch {
      /* fall through — browser blocked unmuted autoplay */
    }
    try {
      video.muted = true;
      await video.play();
      video.muted = false;
      await video.play();
      setNeedsUnmute(false);
    } catch {
      try {
        video.muted = true;
        await video.play();
        setNeedsUnmute(true);
      } catch {
        /* ignore */
      }
    }
  }, []);

  const unmute = useCallback(async () => {
    const video = videoRef.current;
    if (!video) return;
    video.muted = false;
    try {
      await video.play();
      setNeedsUnmute(false);
    } catch {
      setNeedsUnmute(true);
    }
  }, []);

  const toggleFullscreen = useCallback(async () => {
    const el = frameRef.current;
    if (!el) return;
    try {
      if (document.fullscreenElement) {
        await document.exitFullscreen();
      } else {
        await el.requestFullscreen();
      }
    } catch {
      /* ignore */
    }
  }, []);

  const openPopout = useCallback(() => {
    const url = `/preview/${channelId}?pair=${pair}`;
    // Keep opener relationship briefly so the new window can autoplay; session-scoped
    // stop on the engine prevents the modal WS teardown from killing the new preview.
    window.open(url, `roc-preview-${channelId}`, "width=1440,height=860");
    window.setTimeout(() => onCloseRef.current?.(), 150);
  }, [channelId, pair]);

  useEffect(() => {
    let cancelled = false;
    let remoteReady = false;
    const pendingIce: RTCIceCandidateInit[] = [];
    setError(null);
    setNeedsUnmute(false);
    setStatus("Connecting…");

    const pc = new RTCPeerConnection({
      iceServers: [{ urls: "stun:stun.l.google.com:19302" }],
    });
    pc.addTransceiver("video", { direction: "recvonly" });
    pc.addTransceiver("audio", { direction: "recvonly" });

    const attachStream = (stream: MediaStream) => {
      const video = videoRef.current;
      if (!video) return;
      video.srcObject = stream;
      void tryAutostart(variant === "window");
    };

    pc.ontrack = (ev) => {
      if (cancelled) return;
      if (ev.streams[0]) attachStream(ev.streams[0]);
      else {
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
          /* ignore */
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
        if (!remoteReady) pendingIce.push(init);
        else void pc.addIceCandidate(init).catch(() => {});
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

    return () => {
      cancelled = true;
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
      if (videoRef.current) videoRef.current.srcObject = null;
    };
  }, [channelId, pair, session, tryAutostart, variant]);

  useEffect(() => {
    if (noSignal || encodeOff) return;
    if (status === "Live") void tryAutostart(variant === "window");
  }, [encodeOff, noSignal, status, tryAutostart, variant]);

  const live = status === "Live" && !noSignal && !encodeOff;
  const statusLabel = encodeOff
    ? "Off"
    : noSignal && (status === "Live" || status === "DTLS…" || status === "ICE…")
      ? "No signal"
      : status;
  const showOverlay =
    Boolean(error) ||
    status === "Error" ||
    status === "Disconnected" ||
    noSignal ||
    encodeOff;

  return (
    <div className={`preview-player preview-player-${variant}`}>
      <div className="preview-modal-header">
        <div className="preview-modal-title">
          <h2>
            {variant === "window" ? channelName : `Preview · ${channelName}`}
          </h2>
          <span
            className={`preview-modal-status${live ? " live" : ""}${noSignal || encodeOff ? " waiting" : ""}`}
          >
            {statusLabel}
          </span>
          {signalBits && !encodeOff && (
            <span
              className="preview-modal-signal"
              title={
                mezzHint
                  ? mezzDownconvert8
                    ? `HQ → ${mezzHint} (10-bit source downconverted to 8-bit)`
                    : `HQ → ${mezzHint}`
                  : undefined
              }
            >
              {signalBits}
              {mezzHint ? ` · ${mezzHint}` : ""}
              {mezzDownconvert8 ? " · → 8-bit" : ""}
            </span>
          )}
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
          {needsUnmute && !noSignal && !encodeOff && (
            <button
              type="button"
              className="preview-tool-btn preview-unmute-btn"
              onClick={() => void unmute()}
              title="Unmute"
              aria-label="Unmute"
            >
              ♪
            </button>
          )}
          <button
            type="button"
            className="preview-tool-btn"
            onClick={() => void toggleFullscreen()}
            title="Fullscreen"
            aria-label="Fullscreen"
          >
            ⛶
          </button>
          {variant === "modal" && (
            <button
              type="button"
              className="preview-tool-btn"
              onClick={openPopout}
              title="Open in new window"
              aria-label="Open in new window"
            >
              ⧉
            </button>
          )}
          {onClose && (
            <button
              type="button"
              className="modal-close"
              onClick={onClose}
              aria-label="Close"
            >
              ×
            </button>
          )}
        </div>
      </div>
      <div className="preview-modal-stage">
        <AudioMeters channelId={channelId} bus={meterBus} silent={noSignal || encodeOff}>
          <div className="preview-modal-video-wrap">
            <div className="preview-modal-video-frame" ref={frameRef}>
              <video
                ref={videoRef}
                className={`preview-modal-video${noSignal || encodeOff ? " lost" : ""}`}
                playsInline
                autoPlay
                muted={noSignal || encodeOff}
              />
              {needsUnmute && !showOverlay && (
                <button
                  type="button"
                  className="preview-unmute-overlay"
                  onClick={() => void unmute()}
                >
                  Click to enable sound
                </button>
              )}
              {showOverlay && (
                <div className="preview-modal-overlay" role="alert">
                  <p className="preview-modal-overlay-msg">
                    {encodeOff && !error
                      ? "Encode off"
                      : noSignal && !error
                        ? "No signal"
                        : error || (status === "Disconnected" ? "Disconnected" : "Preview failed")}
                  </p>
                  {!noSignal && !encodeOff && (
                    <button type="button" className="preview-modal-reconnect" onClick={reconnect}>
                      Reconnect
                    </button>
                  )}
                </div>
              )}
            </div>
          </div>
        </AudioMeters>
      </div>
    </div>
  );
}
