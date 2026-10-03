"use client";

import { useEffect, useRef } from "react";
import type Hls from "hls.js";
import { mediaURL } from "@/lib/mediaBase";
import { attachHls, stopMedia } from "@/lib/hlsPlayer";
import { useReportPreviewLatency } from "@/lib/previewLatency";

type Props = {
  active: boolean;
  /** Selected stereo pair 0–3, or null when muted. */
  listenPair: number | null;
  /** Absolute path on backend, e.g. /hls/playout/1/preview.m3u8 */
  playlistPath: string;
  /** Remount/reload key when pipeline restarts (e.g. TC stop→start). */
  sessionKey?: string | number;
};

function listenPlaylist(previewPath: string, pair: number): string {
  const q = previewPath.indexOf("?");
  const path = q >= 0 ? previewPath.slice(0, q) : previewPath;
  const query = q >= 0 ? previewPath.slice(q) : "";
  return path.replace(/preview\.m3u8$/, `listen_${pair}.m3u8`) + query;
}

/**
 * Live HLS preview. When a listen pair is selected the engine serves A+V in
 * `listen_N.m3u8` (same H.264 as preview) so the browser keeps lipsync.
 * Muted preview stays on video-only `preview.m3u8`.
 */
export default function HlsPreview({ active, listenPair, playlistPath, sessionKey }: Props) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const videoHls = useRef<Hls | null>(null);
  const reportLatency = useReportPreviewLatency();

  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;

    let cancelled = false;
    let retryTimer: ReturnType<typeof setTimeout> | null = null;

    const stop = () => {
      if (retryTimer != null) {
        clearTimeout(retryTimer);
        retryTimer = null;
      }
      stopMedia(video, videoHls.current);
      videoHls.current = null;
    };

    if (!active) {
      stop();
      return;
    }

    const path =
      listenPair != null ? listenPlaylist(playlistPath, listenPair) : playlistPath;
    const src = mediaURL(path);
    video.muted = listenPair == null;

    const attach = () => {
      if (cancelled || !videoRef.current) return;
      stop();
      videoRef.current.muted = listenPair == null;
      videoHls.current = attachHls(videoRef.current, src, () => {
        if (!cancelled) retryTimer = setTimeout(attach, 1000);
      });
    };

    attach();

    return () => {
      cancelled = true;
      stop();
    };
  }, [active, playlistPath, listenPair, sessionKey]);

  // Drive meter delay from the player's live latency so bars match picture/sound.
  useEffect(() => {
    if (!active || !reportLatency) return;
    const id = window.setInterval(() => {
      const hls = videoHls.current;
      const video = videoRef.current;
      if (!hls || !video) return;
      let sec = hls.latency;
      if (!Number.isFinite(sec) || sec <= 0) {
        const buffered = video.buffered;
        if (buffered.length > 0) {
          sec = buffered.end(buffered.length - 1) - video.currentTime;
        }
      }
      if (Number.isFinite(sec) && sec > 0) reportLatency(sec * 1000);
    }, 250);
    return () => clearInterval(id);
  }, [active, reportLatency]);

  return (
    <>
      {!active && <span className="no-signal" aria-hidden />}
      <video
        ref={videoRef}
        className={`hls-preview${active ? "" : " hls-preview-off"}`}
        playsInline
        muted={listenPair == null}
        autoPlay
      />
    </>
  );
}
