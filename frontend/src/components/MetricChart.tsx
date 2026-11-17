"use client";

import { useMemo, useState } from "react";

export type ChartSeries = {
  key: string;
  label: string;
  /** CSS color (use a --series-* token). */
  color: string;
  values: (number | null | undefined)[];
};

type Props = {
  /** Sample times (ms since epoch), same length as every series' values. */
  times: number[];
  series: ChartSeries[];
  /** Fixed y max (e.g. 100 for percent). Omit to auto-scale to a nice ceiling. */
  yMax?: number;
  format: (v: number) => string;
  height?: number;
  ariaLabel: string;
};

/** Internal x units; the SVG stretches to any width (strokes don't scale). */
const VB_W = 1000;
const PAD_TOP = 6;
const PAD_BOTTOM = 2;

/** Round up to 1 / 2 / 2.5 / 5 × 10^n so the top gridline lands on a clean number. */
function niceCeil(v: number): number {
  if (!(v > 0)) return 1;
  const exp = Math.pow(10, Math.floor(Math.log10(v)));
  for (const m of [1, 2, 2.5, 5, 10]) {
    if (v <= m * exp) return m * exp;
  }
  return 10 * exp;
}

function fmtClock(ms: number): string {
  return new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

/**
 * Small time-series chart: 2px lines, ~10% area wash for a single series,
 * hairline grid at half / top, crosshair + tooltip on hover.
 * Width-agnostic: no measuring, so it is correct at any container size.
 */
export default function MetricChart({ times, series, yMax, format, height = 64, ariaLabel }: Props) {
  const [hover, setHover] = useState<number | null>(null);
  const n = times.length;

  const top = useMemo(() => {
    if (yMax != null) return yMax;
    let m = 0;
    for (const s of series) for (const v of s.values) if (v != null && v > m) m = v;
    return niceCeil(m * 1.1);
  }, [series, yMax]);

  const plotH = height - PAD_TOP - PAD_BOTTOM;
  const xu = (i: number) => (n <= 1 ? VB_W : (i / (n - 1)) * VB_W);
  const xPct = (i: number) => (xu(i) / VB_W) * 100;
  const y = (v: number) => PAD_TOP + plotH - (Math.max(0, Math.min(v, top)) / top) * plotH;

  const paths = useMemo(
    () =>
      series.map((s) => {
        let line = "";
        let area = "";
        let segStart = -1;
        const base = y(0).toFixed(1);
        const closeArea = (end: number) => {
          if (segStart >= 0) area += `L${xu(end).toFixed(1)},${base}L${xu(segStart).toFixed(1)},${base}Z`;
        };
        s.values.forEach((v, i) => {
          if (v == null) {
            closeArea(i - 1);
            segStart = -1;
            return;
          }
          const cmd = segStart < 0 ? "M" : "L";
          if (segStart < 0) segStart = i;
          const pt = `${xu(i).toFixed(1)},${y(v).toFixed(1)}`;
          line += cmd + pt;
          area += cmd + pt;
        });
        closeArea(s.values.length - 1);
        return { key: s.key, color: s.color, line, area };
      }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [series, top, height, n],
  );

  const onMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (n === 0) return;
    const rect = e.currentTarget.getBoundingClientRect();
    if (rect.width <= 0) return;
    const i = Math.round(((e.clientX - rect.left) / rect.width) * (n - 1));
    setHover(Math.max(0, Math.min(n - 1, i)));
  };

  const single = series.length === 1;
  const dotAt = hover ?? n - 1;

  return (
    <div
      className="mchart"
      style={{ height }}
      onPointerMove={onMove}
      onPointerLeave={() => setHover(null)}
      role="img"
      aria-label={ariaLabel}
    >
      <svg viewBox={`0 0 ${VB_W} ${height}`} preserveAspectRatio="none" aria-hidden>
        <line className="mchart-grid" x1={0} x2={VB_W} y1={y(top)} y2={y(top)} vectorEffect="non-scaling-stroke" />
        <line className="mchart-grid" x1={0} x2={VB_W} y1={y(top / 2)} y2={y(top / 2)} vectorEffect="non-scaling-stroke" />
        <line className="mchart-base" x1={0} x2={VB_W} y1={y(0)} y2={y(0)} vectorEffect="non-scaling-stroke" />
        {single &&
          paths.map((p) => <path key={`${p.key}-a`} d={p.area} fill={p.color} fillOpacity={0.12} stroke="none" />)}
        {paths.map((p) => (
          <path
            key={p.key}
            d={p.line}
            fill="none"
            stroke={p.color}
            strokeWidth={2}
            strokeLinejoin="round"
            strokeLinecap="round"
            vectorEffect="non-scaling-stroke"
          />
        ))}
        {hover != null && (
          <line
            className="mchart-cross"
            x1={xu(hover)}
            x2={xu(hover)}
            y1={PAD_TOP}
            y2={y(0)}
            vectorEffect="non-scaling-stroke"
          />
        )}
      </svg>

      <span className="mchart-tick" style={{ top: y(top) + 1 }}>
        {format(top)}
      </span>

      {n > 0 &&
        series.map((s) => {
          const v = s.values[dotAt];
          return v == null ? null : (
            <span
              key={`${s.key}-dot`}
              className="mchart-dot"
              style={{ left: `${xPct(dotAt)}%`, top: y(v), background: s.color }}
              aria-hidden
            />
          );
        })}

      {hover != null && times[hover] != null && (
        <div
          className={`mchart-tip${hover > (n - 1) / 2 ? " left" : ""}`}
          style={{ left: `${xPct(hover)}%` }}
          role="status"
        >
          <div className="mchart-tip-time">{fmtClock(times[hover])}</div>
          {series.map((s) => {
            const v = s.values[hover];
            return (
              <div key={s.key} className="mchart-tip-row">
                <span className="mchart-key" style={{ background: s.color }} aria-hidden />
                <span className="mchart-tip-label">{s.label}</span>
                <span className="mchart-tip-val">{v == null ? "—" : format(v)}</span>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
