"use client";

import { useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { useBodyScrollLock } from "@/hooks/useBodyScrollLock";
import MetricChart, { type ChartSeries } from "@/components/MetricChart";
import {
  fetchSystemHistory,
  type SystemHistPoint,
  type SystemInput,
  type SystemMetrics,
} from "@/lib/api";

type Props = {
  open: boolean;
  metrics: SystemMetrics | null;
  /** Live samples collected by the header pill's poll (newest last). */
  live: SystemHistPoint[];
  onClose: () => void;
};

const RANGES = [
  { id: "5m", label: "5 min", points: 150 },
  { id: "15m", label: "15 min", points: 450 },
] as const;
type RangeId = (typeof RANGES)[number]["id"];

const MAX_POINTS = 450;

function fmtBytes(n?: number): string {
  if (!n || Number.isNaN(n)) return "--";
  const gb = n / (1024 * 1024 * 1024);
  if (gb >= 1024) return `${(gb / 1024).toFixed(1)}T`;
  if (gb >= 1) return `${gb.toFixed(1)}G`;
  return `${(n / (1024 * 1024)).toFixed(0)}M`;
}

function fmtVram(mb?: number): string {
  if (mb == null || Number.isNaN(mb)) return "--";
  return mb >= 1024 ? `${(mb / 1024).toFixed(1)}G` : `${mb.toFixed(0)}M`;
}

const pct = (v?: number) => (v == null || Number.isNaN(v) ? "--" : `${v.toFixed(0)}%`);

function fmtMbps(v?: number | null): string {
  if (v == null || Number.isNaN(v)) return "--";
  if (v >= 1000) return `${(v / 1000).toFixed(2)} Gb/s`;
  if (v >= 100) return `${v.toFixed(0)} Mb/s`;
  return `${v.toFixed(1)} Mb/s`;
}

function fmtDuration(secs?: number): string {
  if (secs == null) return "--";
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m`;
}

type Severity = "good" | "warning" | "critical";
function severity(p: number, warn = 80, crit = 90): Severity {
  return p >= crit ? "critical" : p >= warn ? "warning" : "good";
}
const SEV_ICON: Record<Severity, string> = { good: "●", warning: "▲", critical: "■" };

function Card({
  title,
  value,
  sub,
  children,
  wide,
}: {
  title: string;
  value: string;
  sub?: string;
  children: React.ReactNode;
  wide?: boolean;
}) {
  return (
    <section className={`sysm-card${wide ? " wide" : ""}`} aria-label={title}>
      <header className="sysm-card-head">
        <span className="sysm-card-title">{title}</span>
        <span className="sysm-card-value">{value}</span>
      </header>
      {sub ? <div className="sysm-card-sub">{sub}</div> : null}
      {children}
    </section>
  );
}

function Meter({ label, used, total, detail }: { label: string; used: number; total: number; detail: string }) {
  const p = total > 0 ? (used / total) * 100 : 0;
  const sev = severity(p);
  return (
    <div className="sysm-meter">
      <div className="sysm-meter-head">
        <span className="sysm-meter-label">{label}</span>
        <span className="sysm-meter-detail">{detail}</span>
        <span className={`sysm-sev ${sev}`}>
          <span aria-hidden>{SEV_ICON[sev]}</span> {pct(p)}
        </span>
      </div>
      <div className="sysm-meter-track" role="meter" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(p)} aria-label={label}>
        <div className={`sysm-meter-fill ${sev}`} style={{ width: `${Math.min(100, p)}%` }} />
      </div>
    </div>
  );
}

function SignalCell({ row }: { row: SystemInput }) {
  if (!row.card) return <span className="sysm-muted">n/a</span>;
  return row.card.locked ? (
    <span className="sysm-sev good">
      <span aria-hidden>●</span> Signal
    </span>
  ) : (
    <span className="sysm-muted">
      <span aria-hidden>○</span> No signal
    </span>
  );
}

function pipelineOf(row: SystemInput): { label: string; mode: string | null; sev: Severity | "idle" } {
  if (row.tc && row.tc.status !== "off") {
    return {
      label: `TC ${row.tc.status}`,
      mode: row.tc.mode,
      sev: row.tc.status === "running" ? "good" : row.tc.status === "error" ? "critical" : "warning",
    };
  }
  const st = row.encode.status;
  return {
    label: st === "stopped" ? "Stopped" : `Encode ${st}`,
    mode: st === "stopped" ? null : row.encode.locked_mode,
    sev: st === "running" ? "good" : st === "error" ? "critical" : st === "stopped" ? "idle" : "warning",
  };
}

function InputsTable({ inputs }: { inputs: SystemInput[] }) {
  return (
    <div className="sysm-table-wrap">
      <table className="sysm-table">
        <thead>
          <tr>
            <th scope="col">Ch</th>
            <th scope="col">Name</th>
            <th scope="col">Card input</th>
            <th scope="col">On the wire</th>
            <th scope="col">Pipeline</th>
            <th scope="col">Locked to</th>
            <th scope="col" className="num">Bitrate</th>
            <th scope="col">Out</th>
          </tr>
        </thead>
        <tbody>
          {inputs.map((row) => {
            const pipe = pipelineOf(row);
            const wire = row.card?.locked ? row.card.detected_mode : null;
            const mismatch = wire != null && pipe.mode != null && wire !== pipe.mode;
            return (
              <tr key={row.id}>
                <td className="num">{row.id}</td>
                <td className="sysm-name" title={row.device}>
                  {row.name || `Channel ${row.id}`}
                </td>
                <td>
                  <SignalCell row={row} />
                </td>
                <td>{wire ?? <span className="sysm-muted">—</span>}</td>
                <td>
                  {pipe.sev === "idle" ? (
                    <span className="sysm-muted">{pipe.label}</span>
                  ) : (
                    <span className={`sysm-sev ${pipe.sev}`}>
                      <span aria-hidden>{SEV_ICON[pipe.sev]}</span> {pipe.label}
                    </span>
                  )}
                </td>
                <td>
                  {pipe.mode ?? <span className="sysm-muted">—</span>}
                  {mismatch ? (
                    <span className="sysm-sev warning sysm-mismatch" title="Card sees a different format; the engine re-locks automatically unless recording">
                      <span aria-hidden>▲</span> mismatch
                    </span>
                  ) : null}
                </td>
                <td className="num">
                  {row.encode.bitrate_kbps != null ? fmtMbps(row.encode.bitrate_kbps / 1000) : <span className="sysm-muted">—</span>}
                </td>
                <td>
                  <span className="sysm-flags">
                    {row.encode.recording ? <span className="sysm-flag rec">REC</span> : null}
                    {row.encode.srt ? <span className="sysm-flag">SRT</span> : null}
                  </span>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

export default function SystemStatusModal({ open, metrics: m, live, onClose }: Props) {
  useBodyScrollLock(open);
  const [seed, setSeed] = useState<SystemHistPoint[]>([]);
  const [range, setRange] = useState<RangeId>("5m");

  // Seed from the engine's ring buffer each time the modal opens.
  useEffect(() => {
    if (!open) return;
    let alive = true;
    fetchSystemHistory()
      .then((h) => {
        if (alive) setSeed(h.points.slice(-MAX_POINTS));
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [open]);

  // Engine history up to the open, then live samples from the pill's poll.
  const points = useMemo(() => {
    const lastT = seed.length ? seed[seed.length - 1].t : 0;
    const merged = seed.concat(live.filter((p) => p.t > lastT));
    return merged.length > MAX_POINTS ? merged.slice(-MAX_POINTS) : merged;
  }, [seed, live]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  const view = useMemo(() => {
    const n = RANGES.find((r) => r.id === range)?.points ?? 150;
    return points.slice(-n);
  }, [points, range]);
  const times = useMemo(() => view.map((p) => p.t), [view]);
  const col = <K extends keyof SystemHistPoint>(k: K) => view.map((p) => p[k] as number | undefined);
  const one = (key: keyof SystemHistPoint, label: string): ChartSeries[] => [
    { key, label, color: "var(--series-1)", values: col(key) },
  ];

  // Portal target only exists in the browser.
  if (!open || typeof document === "undefined") return null;

  const gpu = m?.gpu_available;
  const inputs = m?.inputs ?? [];
  const headerBits = [
    m?.hostname,
    m?.host_uptime_secs != null ? `host up ${fmtDuration(m.host_uptime_secs)}` : null,
    m?.engine_uptime_secs != null ? `engine up ${fmtDuration(m.engine_uptime_secs)}` : null,
    m?.gpu_name ? `${m.gpu_name}${m.gpu_driver ? ` · driver ${m.gpu_driver}` : ""}` : null,
  ].filter(Boolean);

  return createPortal(
    <div className="modal-backdrop" onClick={onClose} role="presentation">
      <div
        className="modal-panel sysm-root"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-label="System status"
      >
        <div className="modal-header sysm-header">
          <div>
            <h2>System status</h2>
            <div className="sysm-subtitle">{headerBits.join(" · ")}</div>
          </div>
          <div className="sysm-header-right">
            <div className="sysm-range" role="group" aria-label="Time range">
              {RANGES.map((r) => (
                <button
                  key={r.id}
                  type="button"
                  className={range === r.id ? "active" : ""}
                  aria-pressed={range === r.id}
                  onClick={() => setRange(r.id)}
                >
                  {r.label}
                </button>
              ))}
            </div>
            <button type="button" className="modal-close" onClick={onClose} aria-label="Close">
              ×
            </button>
          </div>
        </div>

        {!m ? (
          <div className="sysm-muted">Loading…</div>
        ) : (
          <>
            <div className="sysm-grid">
              <Card
                title="CPU"
                value={pct(m.cpu_percent)}
                sub={
                  m.load_avg
                    ? `load ${m.load_avg.map((v) => v.toFixed(2)).join(" / ")}${m.cpu_cores ? ` · ${m.cpu_cores} cores` : ""}`
                    : undefined
                }
              >
                <MetricChart times={times} series={one("cpu", "CPU")} yMax={100} format={pct} ariaLabel="CPU usage, last minutes" />
              </Card>
              <Card title="Memory" value={pct(m.mem_percent)} sub={`${fmtBytes(m.mem_used_bytes)} / ${fmtBytes(m.mem_total_bytes)}`}>
                <MetricChart times={times} series={one("mem", "Memory")} yMax={100} format={pct} ariaLabel="Memory usage" />
              </Card>
              <Card
                title="GPU"
                value={gpu ? pct(m.gpu_percent) : "n/a"}
                sub={
                  gpu
                    ? [
                        m.gpu_temp_c != null ? `${m.gpu_temp_c.toFixed(0)} °C` : null,
                        m.gpu_power_w != null
                          ? `${m.gpu_power_w.toFixed(0)} W${m.gpu_power_limit_w ? ` / ${m.gpu_power_limit_w.toFixed(0)} W` : ""}`
                          : null,
                        `NVDEC ${pct(m.nvdec_percent)}`,
                      ]
                        .filter(Boolean)
                        .join(" · ")
                    : undefined
                }
              >
                <MetricChart times={times} series={one("gpu", "GPU")} yMax={100} format={pct} ariaLabel="GPU compute utilisation" />
              </Card>
              <Card
                title="VRAM"
                value={gpu ? `${fmtVram(m.gpu_mem_used_mb)} / ${fmtVram(m.gpu_mem_total_mb)}` : "n/a"}
              >
                <MetricChart times={times} series={one("vram", "VRAM")} yMax={100} format={pct} ariaLabel="GPU memory used" />
              </Card>
              <Card
                title="NVENC"
                value={gpu ? pct(m.nvenc_percent) : "n/a"}
                sub={
                  gpu
                    ? [
                        m.nvenc_used != null && m.nvenc_limit != null ? `${m.nvenc_used}/${m.nvenc_limit} engine slots` : null,
                        m.gpu_encoder_sessions != null ? `${m.gpu_encoder_sessions} sessions` : null,
                        m.gpu_encoder_fps ? `${m.gpu_encoder_fps.toFixed(0)} fps avg` : null,
                      ]
                        .filter(Boolean)
                        .join(" · ")
                    : "NVIDIA GPU not available"
                }
              >
                <MetricChart times={times} series={one("nvenc", "NVENC")} yMax={100} format={pct} ariaLabel="NVENC encoder utilisation" />
              </Card>
              <Card
                title="Output"
                value={fmtMbps((m.encode_kbps_total ?? 0) / 1000)}
                sub={`encode total · SRT ${fmtMbps((m.srt_kbps_total ?? 0) / 1000)}`}
              >
                <MetricChart times={times} series={one("enc_mbps", "Encode")} format={fmtMbps} ariaLabel="Total encode bitrate" />
              </Card>
              <Card
                title="Network"
                wide
                value={`↓ ${fmtMbps(m.net_rx_bps != null ? m.net_rx_bps / 1e6 : null)}  ↑ ${fmtMbps(
                  m.net_tx_bps != null ? m.net_tx_bps / 1e6 : null,
                )}`}
                sub={m.net_ifaces?.length ? `host NICs: ${m.net_ifaces.join(", ")} (DeckLink IP traffic is on the card)` : undefined}
              >
                <div className="sysm-legend" aria-hidden>
                  <span>
                    <span className="mchart-key" style={{ background: "var(--series-1)" }} /> In
                  </span>
                  <span>
                    <span className="mchart-key" style={{ background: "var(--series-2)" }} /> Out
                  </span>
                </div>
                <MetricChart
                  times={times}
                  series={[
                    { key: "rx", label: "In", color: "var(--series-1)", values: col("rx_mbps") },
                    { key: "tx", label: "Out", color: "var(--series-2)", values: col("tx_mbps") },
                  ]}
                  format={fmtMbps}
                  ariaLabel="Network throughput in and out"
                />
              </Card>
            </div>

            <Meter
              label="Recordings disk"
              used={m.disk_used_bytes}
              total={m.disk_total_bytes}
              detail={`${m.disk_path ?? ""} · ${fmtBytes(m.disk_used_bytes)} / ${fmtBytes(m.disk_total_bytes)} · ${fmtBytes(
                Math.max(0, m.disk_total_bytes - m.disk_used_bytes),
              )} free`}
            />

            <h3 className="sysm-section">Inputs</h3>
            {inputs.length ? <InputsTable inputs={inputs} /> : <div className="sysm-muted">No channel data.</div>}
          </>
        )}
      </div>
    </div>,
    document.body,
  );
}
