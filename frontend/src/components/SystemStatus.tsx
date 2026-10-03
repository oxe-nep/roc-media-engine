"use client";

import { useEffect, useRef, useState } from "react";
import { fetchSystemMetrics, type SystemMetrics } from "@/lib/api";

function fmtBytes(n: number): string {
  if (!n || Number.isNaN(n)) return "--";
  const gb = n / (1024 * 1024 * 1024);
  if (gb >= 1) return `${gb.toFixed(1)}G`;
  const mb = n / (1024 * 1024);
  return `${mb.toFixed(0)}M`;
}

function fmtVram(mb?: number): string {
  if (mb == null || Number.isNaN(mb)) return "--";
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)}G`;
  return `${mb.toFixed(0)}M`;
}

function pct(n?: number): string {
  if (n == null || Number.isNaN(n)) return "--";
  return `${n.toFixed(0)}%`;
}

function Item({
  label,
  value,
  sub,
  title,
}: {
  label: string;
  value: string;
  sub?: string;
  title?: string;
}) {
  return (
    <span className="sys-item" title={title}>
      <span className="sys-label">{label}</span>
      <span className="sys-value">
        {value}
        {sub ? <span className="sys-sub">{sub}</span> : null}
      </span>
    </span>
  );
}

export default function SystemStatus() {
  const [m, setM] = useState<SystemMetrics | null>(null);
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let alive = true;
    const load = async () => {
      try {
        const data = await fetchSystemMetrics();
        if (alive) setM(data);
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

  useEffect(() => {
    if (!open) return;
    const onPointer = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("mousedown", onPointer);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onPointer);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  if (!m) {
    return (
      <button type="button" className="sys-pill muted" disabled>
        Host…
      </button>
    );
  }

  const hostSummary = `Host ${pct(m.cpu_percent)}`;
  const gpuSummary = m.gpu_available ? `GPU ${pct(m.nvenc_percent)}` : "GPU —";

  return (
    <div className="sys-status-wrap" ref={rootRef}>
      <button
        type="button"
        className={`sys-pill${open ? " open" : ""}`}
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        aria-haspopup="dialog"
        title="Host & GPU status"
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
          <span className="sys-pill-value">
            {m.gpu_available ? pct(m.nvenc_percent) : "—"}
          </span>
        </span>
      </button>

      {open && (
        <div
          className="sys-popover"
          role="dialog"
          aria-label={`${hostSummary} · ${gpuSummary}`}
        >
          <div className="sys-group" aria-label="Host">
            <span className="sys-group-label">Host</span>
            <Item label="CPU" value={pct(m.cpu_percent)} />
            <Item
              label="MEM"
              value={pct(m.mem_percent)}
              sub={`${fmtBytes(m.mem_used_bytes)}/${fmtBytes(m.mem_total_bytes)}`}
            />
            <Item
              label="DISK"
              value={pct(m.disk_percent)}
              sub={`${fmtBytes(m.disk_used_bytes)}/${fmtBytes(m.disk_total_bytes)}`}
              title={m.disk_path ? `Disk: ${m.disk_path}` : "Disk"}
            />
          </div>

          <div
            className={`sys-group${m.gpu_available ? "" : " muted"}`}
            aria-label="GPU"
            title={
              m.gpu_available
                ? `GPU compute ${pct(m.gpu_percent)} · VRAM ${fmtVram(m.gpu_mem_used_mb)}/${fmtVram(m.gpu_mem_total_mb)}`
                : "NVIDIA GPU not available"
            }
          >
            <span className="sys-group-label">GPU</span>
            {m.gpu_available ? (
              <>
                <Item label="NVENC" value={pct(m.nvenc_percent)} title="Encoder utilization" />
                <Item label="NVDEC" value={pct(m.nvdec_percent)} title="Decoder utilization" />
                <Item
                  label="VRAM"
                  value={`${fmtVram(m.gpu_mem_used_mb)}/${fmtVram(m.gpu_mem_total_mb)}`}
                  title={`GPU compute ${pct(m.gpu_percent)}`}
                />
              </>
            ) : (
              <Item label="—" value="n/a" />
            )}
          </div>
        </div>
      )}
    </div>
  );
}
