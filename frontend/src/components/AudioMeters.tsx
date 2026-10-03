"use client";

import { useEffect, useRef, type ReactNode } from "react";
import { useMeterLevels, type MeterBus } from "@/hooks/useDashboard";
import {
  PreviewLatencyProvider,
  useDelayedMeterLevels,
  usePreviewDelayMs,
} from "@/lib/previewLatency";

const METER_SEGMENTS = 24;
const METER_MIN_DB = -50;
const METER_MAX_DB = 0;
const SEG_GAP = 1;
const OFF = "rgba(255, 255, 255, 0.07)";
const GREEN = "#3fa34a";
const YELLOW = "#d4a017";
const RED = "#e74c3c";

function hasAudioLevel(db?: number): db is number {
  return db !== undefined && !Number.isNaN(db) && db > -89;
}

function segmentZone(index: number): "green" | "yellow" | "red" {
  const db = METER_MIN_DB + ((index + 1) / METER_SEGMENTS) * (METER_MAX_DB - METER_MIN_DB);
  if (db <= -18) return "green";
  if (db <= -9) return "yellow";
  return "red";
}

function litSegmentCount(db?: number): number {
  if (!hasAudioLevel(db)) return 0;
  const clamped = Math.max(METER_MIN_DB, Math.min(METER_MAX_DB, db));
  const pct = (clamped - METER_MIN_DB) / (METER_MAX_DB - METER_MIN_DB);
  return Math.round(pct * METER_SEGMENTS);
}

function meterChannels(a?: { l: number; r: number; channels?: number[] }): number[] {
  const out = Array.from({ length: 8 }, () => -90);
  if (a?.channels?.length) {
    for (let i = 0; i < 8 && i < a.channels.length; i++) out[i] = a.channels[i];
    return out;
  }
  if (a) {
    out[0] = a.l;
    out[1] = a.r;
  }
  return out;
}

function zoneColor(zone: "green" | "yellow" | "red"): string {
  if (zone === "green") return GREEN;
  if (zone === "yellow") return YELLOW;
  return RED;
}

function drawColumn(canvas: HTMLCanvasElement, db: number) {
  const dpr = window.devicePixelRatio || 1;
  const w = canvas.clientWidth;
  const h = canvas.clientHeight;
  if (w < 1 || h < 1) return;
  const pixelW = Math.round(w * dpr);
  const pixelH = Math.round(h * dpr);
  if (canvas.width !== pixelW || canvas.height !== pixelH) {
    canvas.width = pixelW;
    canvas.height = pixelH;
  }
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, w, h);
  const lit = litSegmentCount(db);
  const segH = (h - SEG_GAP * (METER_SEGMENTS - 1)) / METER_SEGMENTS;
  for (let i = 0; i < METER_SEGMENTS; i++) {
    const y = h - (i + 1) * segH - i * SEG_GAP;
    ctx.fillStyle = i < lit ? zoneColor(segmentZone(i)) : OFF;
    const hh = Math.max(1, segH);
    if (typeof ctx.roundRect === "function") {
      ctx.beginPath();
      ctx.roundRect(0, y, w, hh, 1);
      ctx.fill();
    } else {
      ctx.fillRect(0, y, w, hh);
    }
  }
}

function MeterColumn({ db, label }: { db: number; label: string }) {
  const colRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const dbRef = useRef(db);
  dbRef.current = db;

  useEffect(() => {
    const col = colRef.current;
    const canvas = canvasRef.current;
    if (!col || !canvas) return;
    const paint = () => drawColumn(canvas, dbRef.current);
    paint();
    const ro = new ResizeObserver(paint);
    ro.observe(col);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (canvas) drawColumn(canvas, db);
  }, [db]);

  const tip = hasAudioLevel(db) ? `${db.toFixed(1)} dBFS` : "— dBFS";

  return (
    <div ref={colRef} className="audio-col" title={`Ch ${label}: ${tip}`}>
      <canvas ref={canvasRef} className="audio-meter-canvas" aria-hidden />
      <span className="audio-label">{label}</span>
    </div>
  );
}

function MeterBank({ dbs, labels, title }: { dbs: number[]; labels: string[]; title: string }) {
  const tip = labels
    .map((label, i) => {
      const db = dbs[i];
      const text = hasAudioLevel(db) ? `${db.toFixed(1)} dBFS` : "— dBFS";
      return `${label}: ${text}`;
    })
    .join(" · ");

  return (
    <div className="audio-bank audio-bank-cols" title={`${title}. Green ≤ -18 · Yellow ≤ -9 · Red above -9. ${tip}`}>
      {dbs.map((db, i) => (
        <MeterColumn key={labels[i]} db={db} label={labels[i]} />
      ))}
    </div>
  );
}

function AudioMetersInner({
  channelId,
  bus,
  children,
  channels = 8,
}: {
  channelId: number;
  bus: MeterBus;
  children: ReactNode;
  channels?: 2 | 8;
}) {
  const live = useMeterLevels(channelId, bus);
  const delayMs = usePreviewDelayMs();
  const levels = useDelayedMeterLevels(live, delayMs);
  const ch = meterChannels(levels);
  if (channels === 2) {
    return (
      <>
        <div className="audio-meter audio-meter-left audio-meter-stereo">
          <MeterBank dbs={ch.slice(0, 2)} labels={["1", "2"]} title="Audio 1–2" />
        </div>
        {children}
      </>
    );
  }
  return (
    <>
      <div className="audio-meter audio-meter-left audio-meter-8ch">
        <MeterBank dbs={ch.slice(0, 4)} labels={["1", "2", "3", "4"]} title="Audio 1–4" />
      </div>
      {children}
      <div className="audio-meter audio-meter-right audio-meter-8ch">
        <MeterBank dbs={ch.slice(4, 8)} labels={["5", "6", "7", "8"]} title="Audio 5–8" />
      </div>
    </>
  );
}

/** Peak meters delayed to match HLS preview latency (live peaks are ~seconds ahead). */
export default function AudioMeters(props: {
  channelId: number;
  bus: MeterBus;
  children: ReactNode;
  /** Decode/playout is stereo; encode and TC keep 8 meters. */
  channels?: 2 | 8;
}) {
  return (
    <PreviewLatencyProvider>
      <AudioMetersInner {...props} />
    </PreviewLatencyProvider>
  );
}
