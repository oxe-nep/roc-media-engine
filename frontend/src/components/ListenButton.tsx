"use client";

import { useEffect, useRef, useState } from "react";

export const LISTEN_PAIRS = [
  { id: 0, label: "1–2" },
  { id: 1, label: "3–4" },
  { id: 2, label: "5–6" },
  { id: 3, label: "7–8" },
] as const;

type Props = {
  pair: number | null;
  onChange: (pair: number | null) => void;
  disabled?: boolean;
  /** Decode only has pair 1–2; encode/TC keep all four. */
  stereoOnly?: boolean;
};

export default function ListenButton({ pair, onChange, disabled, stereoOnly }: Props) {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const listening = pair != null;
  const label = listening ? LISTEN_PAIRS[pair]?.label ?? "1–2" : "";

  useEffect(() => {
    if (!open || stereoOnly) return;
    const onDoc = (e: MouseEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [open, stereoOnly]);

  if (stereoOnly) {
    return (
      <button
        type="button"
        className={`badge listen-btn ${listening ? "active" : ""}`}
        disabled={disabled}
        onClick={() => onChange(listening ? null : 0)}
        title={listening ? "Listening 1–2" : "Listen 1–2"}
      >
        {listening ? "🔊" : "🔈"}
      </button>
    );
  }

  const pick = (id: number) => {
    onChange(pair === id ? null : id);
    setOpen(false);
  };

  return (
    <div className="listen-wrap" ref={wrapRef}>
      <button
        type="button"
        className={`badge listen-btn ${listening ? "active" : ""}`}
        disabled={disabled}
        onClick={() => setOpen((v) => !v)}
        title={listening ? `Listening ${label}` : "Listen — choose pair"}
        aria-expanded={open}
        aria-haspopup="menu"
      >
        {listening ? "🔊" : "🔈"}
      </button>
      {open && (
        <div className="listen-menu" role="menu">
          {LISTEN_PAIRS.map((p) => (
            <button
              key={p.id}
              type="button"
              role="menuitem"
              className={pair === p.id ? "on" : ""}
              onClick={() => pick(p.id)}
            >
              {p.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
