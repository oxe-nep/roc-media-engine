"use client";

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import type { AudioLevels } from "@/lib/api";

/** Fallback when hls.js has not reported latency yet (~3×1s segments). */
export const DEFAULT_PREVIEW_LATENCY_MS = 3000;

type SetLatency = (ms: number) => void;

const PreviewLatencyContext = createContext<SetLatency | null>(null);

/** Parent around thumb + meters: children report HLS latency, meters delay to match. */
export function PreviewLatencyProvider({ children }: { children: ReactNode }) {
  const [delayMs, setDelayMs] = useState(DEFAULT_PREVIEW_LATENCY_MS);
  const setLatency = useCallback((ms: number) => {
    if (!Number.isFinite(ms) || ms < 0) return;
    const clamped = Math.min(15_000, Math.max(500, Math.round(ms)));
    setDelayMs((prev) => (Math.abs(prev - clamped) < 80 ? prev : clamped));
  }, []);

  return (
    <PreviewLatencyContext.Provider value={setLatency}>
      <PreviewDelayContext.Provider value={delayMs}>{children}</PreviewDelayContext.Provider>
    </PreviewLatencyContext.Provider>
  );
}

const PreviewDelayContext = createContext(DEFAULT_PREVIEW_LATENCY_MS);

export function useReportPreviewLatency(): SetLatency | null {
  return useContext(PreviewLatencyContext);
}

export function usePreviewDelayMs(): number {
  return useContext(PreviewDelayContext);
}

type Sample = { t: number; levels: AudioLevels };

/** Replay meter peaks delayed by HLS preview latency so bars match picture/sound. */
export function useDelayedMeterLevels(
  levels: AudioLevels | undefined,
  delayMs: number,
): AudioLevels | undefined {
  const buf = useRef<Sample[]>([]);
  const [out, setOut] = useState<AudioLevels | undefined>(levels);

  useEffect(() => {
    if (!levels) {
      buf.current = [];
      setOut(undefined);
      return;
    }
    const now = performance.now();
    buf.current.push({ t: now, levels });
    const keepFrom = now - delayMs - 500;
    while (buf.current.length > 1 && buf.current[0].t < keepFrom) {
      buf.current.shift();
    }
  }, [levels, delayMs]);

  useEffect(() => {
    let raf = 0;
    const tick = () => {
      const target = performance.now() - delayMs;
      const q = buf.current;
      let pick: AudioLevels | undefined;
      for (let i = q.length - 1; i >= 0; i--) {
        if (q[i].t <= target) {
          pick = q[i].levels;
          break;
        }
      }
      if (pick) setOut(pick);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [delayMs]);

  return out;
}
