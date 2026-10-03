"use client";

import { useEffect, useRef } from "react";
import type Hls from "hls.js";
import { attachHls, stopMedia } from "@/lib/hlsPlayer";
import { attachHlsMeter, type HlsMeterTap, type StereoPeaks } from "@/lib/hlsAudioMeter";

type Props = {
  active: boolean;
  src: string;
  /** Called each animation frame with L/R dBFS, or null when inactive. */
  onPeaks: (peaks: StereoPeaks | null) => void;
};

/**
 * Muted listen HLS tap (no picture) so the other stereo pairs can be metered
 * while the visible preview plays a different listen_N.
 */
export default function HiddenListenMeter({ active, src, onPeaks }: Props) {
  const audioRef = useRef<HTMLAudioElement>(null);
  const hlsRef = useRef<Hls | null>(null);
  const tapRef = useRef<HlsMeterTap | null>(null);
  const onPeaksRef = useRef(onPeaks);
  onPeaksRef.current = onPeaks;

  useEffect(() => {
    const el = audioRef.current;
    if (!el) return;

    let cancelled = false;
    let retryTimer: ReturnType<typeof setTimeout> | null = null;
    let readyTimer: ReturnType<typeof setTimeout> | null = null;
    let raf = 0;

    const clearTimers = () => {
      if (retryTimer != null) {
        clearTimeout(retryTimer);
        retryTimer = null;
      }
      if (readyTimer != null) {
        clearTimeout(readyTimer);
        readyTimer = null;
      }
      cancelAnimationFrame(raf);
      raf = 0;
    };

    const tearDownTap = () => {
      tapRef.current?.disconnect();
      tapRef.current = null;
      onPeaksRef.current(null);
    };

    const tearDown = () => {
      clearTimers();
      tearDownTap();
      stopMedia(el, hlsRef.current);
      hlsRef.current = null;
    };

    if (!active) {
      tearDown();
      return;
    }

    const startMeter = () => {
      if (cancelled || !audioRef.current) return;
      try {
        tapRef.current = attachHlsMeter(audioRef.current, false);
      } catch {
        readyTimer = setTimeout(startMeter, 400);
        return;
      }
      const tick = () => {
        if (cancelled || !tapRef.current) return;
        tapRef.current.setAudible(false);
        onPeaksRef.current(tapRef.current.sample());
        raf = requestAnimationFrame(tick);
      };
      raf = requestAnimationFrame(tick);
    };

    const attach = () => {
      if (cancelled || !audioRef.current) return;
      tearDownTap();
      clearTimers();
      stopMedia(audioRef.current, hlsRef.current);
      hlsRef.current = null;
      audioRef.current.muted = true;
      hlsRef.current = attachHls(audioRef.current, src, () => {
        if (!cancelled) retryTimer = setTimeout(attach, 1000);
      });
      readyTimer = setTimeout(startMeter, 200);
    };

    attach();

    return () => {
      cancelled = true;
      tearDown();
    };
  }, [active, src]);

  return (
    <audio
      ref={audioRef}
      className="hls-meter-tap"
      playsInline
      muted
      autoPlay
      preload="none"
      aria-hidden
    />
  );
}
