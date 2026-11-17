"use client";

import { useCallback, useEffect, useState } from "react";
import { fetchSystemMetrics, histPointFromMetrics, type SystemHistPoint, type SystemMetrics } from "@/lib/api";
import SystemStatusModal from "@/components/SystemStatusModal";

function pct(n?: number): string {
  if (n == null || Number.isNaN(n)) return "--";
  return `${n.toFixed(0)}%`;
}

/** Header pill (Host / GPU) — opens the full system status modal. */
export default function SystemStatus() {
  const [m, setM] = useState<SystemMetrics | null>(null);
  const [open, setOpen] = useState(false);
  const [live, setLive] = useState<SystemHistPoint[]>([]);
  const close = useCallback(() => setOpen(false), []);

  useEffect(() => {
    let alive = true;
    const load = async () => {
      try {
        const data = await fetchSystemMetrics();
        if (!alive) return;
        setM(data);
        const p = histPointFromMetrics(data);
        if (p) {
          setLive((prev) =>
            prev.length && prev[prev.length - 1].t >= p.t ? prev : [...prev, p].slice(-450),
          );
        }
      } catch {
        // ignore transient errors
      }
    };
    load();
    const t = setInterval(load, 2000);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, []);

  if (!m) {
    return (
      <button type="button" className="sys-pill muted" disabled>
        Host…
      </button>
    );
  }

  return (
    <div className="sys-status-wrap">
      <button
        type="button"
        className={`sys-pill${open ? " open" : ""}`}
        onClick={() => setOpen(true)}
        aria-haspopup="dialog"
        title="System status"
      >
        <span className="sys-pill-part">
          <span className="sys-pill-label">Host</span>
          <span className="sys-pill-value">{pct(m.cpu_percent)}</span>
        </span>
        <span className="sys-pill-sep" aria-hidden>
          ·
        </span>
        <span className="sys-pill-part">
          <span className="sys-pill-label">GPU</span>
          <span className="sys-pill-value">{m.gpu_available ? pct(m.nvenc_percent) : "—"}</span>
        </span>
      </button>
      <SystemStatusModal open={open} metrics={m} live={live} onClose={close} />
    </div>
  );
}
