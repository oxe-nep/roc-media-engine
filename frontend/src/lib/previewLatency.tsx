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
import type { StereoPeaks } from "@/lib/hlsAudioMeter";

/** Fallback when hls.js has not reported latency yet (~3×1s segments). */
export const DEFAULT_PREVIEW_LATENCY_MS = 3000;

export const HLS_METER_PAIR_COUNT = 4;

type SetLatency = (ms: number) => void;

/** Per listen-pair peaks from muted/unmuted HLS taps (index 0 = ch 1–2, …). */
export type HlsMeterBanks = (StereoPeaks | null)[];

type SetHlsMeters = (banks: HlsMeterBanks) => void;

const PreviewLatencyContext = createContext<SetLatency | null>(null);
const HlsMeterReportContext = createContext<SetHlsMeters | null>(null);
const HlsMeterBanksContext = createContext<HlsMeterBanks>(emptyBanks());
const PreviewDelayContext = createContext(DEFAULT_PREVIEW_LATENCY_MS);

function emptyBanks(): HlsMeterBanks {
  return [null, null, null, null];
}

/** Parent around thumb + meters: children report HLS latency / listen peaks. */
export function PreviewLatencyProvider({ children }: { children: ReactNode }) {
  const [delayMs, setDelayMs] = useState(DEFAULT_PREVIEW_LATENCY_MS);
  const [hlsBanks, setHlsBanks] = useState<HlsMeterBanks>(emptyBanks);
  const setLatency = useCallback((ms: number) => {
    if (!Number.isFinite(ms) || ms < 0) return;
    const clamped = Math.min(15_000, Math.max(500, Math.round(ms)));
    setDelayMs((prev) => (Math.abs(prev - clamped) < 80 ? prev : clamped));
  }, []);

  const reportMeters = useCallback((banks: HlsMeterBanks) => {
    setHlsBanks((prev) => {
      let same = prev.length === banks.length;
      if (same) {
        for (let i = 0; i < banks.length; i++) {
          const a = prev[i];
          const b = banks[i];
          if (a === b) continue;
          if (!a || !b || a.l !== b.l || a.r !== b.r) {
            same = false;
            break;
          }
        }
      }
      return same ? prev : banks.slice(0, HLS_METER_PAIR_COUNT);
    });
  }, []);

  return (
    <PreviewLatencyContext.Provider value={setLatency}>
      <HlsMeterReportContext.Provider value={reportMeters}>
        <HlsMeterBanksContext.Provider value={hlsBanks}>
          <PreviewDelayContext.Provider value={delayMs}>{children}</PreviewDelayContext.Provider>
        </HlsMeterBanksContext.Provider>
      </HlsMeterReportContext.Provider>
    </PreviewLatencyContext.Provider>
  );
}

export function useReportPreviewLatency(): SetLatency | null {
  return useContext(PreviewLatencyContext);
}

export function useReportHlsMeters(): SetHlsMeters | null {
  return useContext(HlsMeterReportContext);
}

export function useHlsMeterBanks(): HlsMeterBanks {
  return useContext(HlsMeterBanksContext);
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
