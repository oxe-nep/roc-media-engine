"use client";

import { useEffect, useRef } from "react";
import type Hls from "hls.js";
import HiddenListenMeter from "@/components/HiddenListenMeter";
import { mediaURL } from "@/lib/mediaBase";
import { attachHls, stopMedia } from "@/lib/hlsPlayer";
import {
  attachHlsMeter,
  unlockHlsAudio,
  type StereoPeaks,
} from "@/lib/hlsAudioMeter";
import {
  HLS_METER_PAIR_COUNT,
  useReportHlsMeters,
  useReportPreviewLatency,
} from "@/lib/previewLatency";

type Props = {
  active: boolean;
  /** Selected stereo pair 0–3, or null when monitoring muted. */
  listenPair: number | null;
  /** Absolute path on backend, e.g. /hls/1/preview.m3u8 (rewritten to listen_N). */
  playlistPath: string;
  /** Remount/reload key when pipeline restarts (e.g. TC stop→start). */
  sessionKey?: string | number;
  /** How many listen pairs to meter (encode/TC: 4, decode stereo: 1). */
  meterPairs?: number;
};

function listenPlaylist(previewPath: string, pair: number): string {
  const q = previewPath.indexOf("?");
  const path = q >= 0 ? previewPath.slice(0, q) : previewPath;
  const query = q >= 0 ? previewPath.slice(q) : "";
  return path.replace(/preview\.m3u8$/, `listen_${pair}.m3u8`) + query;
}

/**
 * Visible preview always plays A+V `listen_N`. Extra muted `listen_*` taps meter
 * the other stereo pairs so all 8 channels track the HLS timeline. GainNode
 * unmutes only the selected listen pair.
 */
export default function HlsPreview({
  active,
  listenPair,
  playlistPath,
  sessionKey,
  meterPairs = HLS_METER_PAIR_COUNT,
}: Props) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const videoHls = useRef<Hls | null>(null);
  const stickyPair = useRef(0);
  const peaksRef = useRef<(StereoPeaks | null)[]>([null, null, null, null]);
  const reportLatency = useReportPreviewLatency();
  const reportMeters = useReportHlsMeters();

  const pairCount = Math.max(1, Math.min(HLS_METER_PAIR_COUNT, meterPairs));

  useEffect(() => {
    if (listenPair != null) stickyPair.current = listenPair;
  }, [listenPair]);

  const playPair = Math.min(listenPair ?? stickyPair.current, pairCount - 1);
  const audible = listenPair != null;

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

    const src = mediaURL(listenPlaylist(playlistPath, playPair));
    video.muted = true;

    const attach = () => {
      if (cancelled || !videoRef.current) return;
      stop();
      videoRef.current.muted = true;
      videoHls.current = attachHls(videoRef.current, src, () => {
        if (!cancelled) retryTimer = setTimeout(attach, 1000);
      });
    };

    attach();

    return () => {
      cancelled = true;
      stop();
    };
  }, [active, playlistPath, playPair, sessionKey]);

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
      const PIPELINE_MS = 1200;
      if (Number.isFinite(sec) && sec > 0) reportLatency(sec * 1000 + PIPELINE_MS);
    }, 250);
    return () => clearInterval(id);
  }, [active, reportLatency]);

  // Visible listen stream → peaks for playPair; gain controls forlyssning.
  useEffect(() => {
    if (!active) {
      peaksRef.current[playPair] = null;
      return;
    }
    const video = videoRef.current;
    if (!video) return;

    let tap: ReturnType<typeof attachHlsMeter> | null = null;
    let raf = 0;
    let cancelled = false;

    const start = () => {
      if (cancelled || !videoRef.current) return;
      try {
        unlockHlsAudio();
        tap = attachHlsMeter(videoRef.current, audible);
      } catch {
        window.setTimeout(start, 300);
        return;
      }
      const tick = () => {
        if (cancelled || !tap) return;
        tap.setAudible(audible);
        peaksRef.current[playPair] = tap.sample();
        raf = requestAnimationFrame(tick);
      };
      raf = requestAnimationFrame(tick);
    };

    const readyTimer = window.setTimeout(start, 150);

    return () => {
      cancelled = true;
      clearTimeout(readyTimer);
      cancelAnimationFrame(raf);
      tap?.disconnect();
      peaksRef.current[playPair] = null;
    };
  }, [active, playPair, audible, sessionKey]);

  // Publish aggregated 4-pair banks (~25 Hz — enough for LED meters).
  useEffect(() => {
    if (!reportMeters) return;
    if (!active) {
      reportMeters([null, null, null, null]);
      return;
    }
    let raf = 0;
    let last = 0;
    const tick = (t: number) => {
      if (t - last >= 40) {
        last = t;
        reportMeters(peaksRef.current.slice(0, HLS_METER_PAIR_COUNT));
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => {
      cancelAnimationFrame(raf);
      reportMeters([null, null, null, null]);
    };
  }, [active, reportMeters]);

  const hiddenPairs = Array.from({ length: pairCount }, (_, i) => i).filter((p) => p !== playPair);

  return (
    <>
      {!active && <span className="no-signal" aria-hidden />}
      <video
        ref={videoRef}
        className={`hls-preview${active ? "" : " hls-preview-off"}`}
        playsInline
        muted
        autoPlay
      />
      {hiddenPairs.map((p) => (
        <HiddenListenMeter
          key={`m${p}-${sessionKey ?? 0}`}
          active={active}
          src={mediaURL(listenPlaylist(playlistPath, p))}
          onPeaks={(peaks) => {
            peaksRef.current[p] = peaks;
          }}
        />
      ))}
    </>
  );
}
