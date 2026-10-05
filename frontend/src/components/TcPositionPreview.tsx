"use client";

import { useCallback, useRef } from "react";
import type { TcLoopPosition } from "@/lib/api";

type Props = {
  x: number;
  y: number;
  fontsize: number;
  opacity: number;
  disabled?: boolean;
  onMove: (x: number, y: number) => void;
  onSnap?: (pos: TcLoopPosition) => void;
};

const SNAPS: { id: TcLoopPosition; label: string; x: number; y: number }[] = [
  { id: "top_left", label: "TL", x: 0.04, y: 0.04 },
  { id: "top_right", label: "TR", x: 0.72, y: 0.04 },
  { id: "center", label: "C", x: 0.38, y: 0.44 },
  { id: "bottom_left", label: "BL", x: 0.04, y: 0.86 },
  { id: "bottom_right", label: "BR", x: 0.72, y: 0.86 },
];

/** Preview frame is 16:9; map fontsize(px @1080) → CSS px in the preview. */
function previewFontPx(fontsize: number, frameHeight: number) {
  return Math.max(6, (fontsize / 1080) * frameHeight);
}

export default function TcPositionPreview({
  x,
  y,
  fontsize,
  opacity,
  disabled,
  onMove,
  onSnap,
}: Props) {
  const frameRef = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);

  const setFromClient = useCallback(
    (clientX: number, clientY: number) => {
      const el = frameRef.current;
      if (!el) return;
      const r = el.getBoundingClientRect();
      if (r.width < 1 || r.height < 1) return;
      const nx = Math.min(1, Math.max(0, (clientX - r.left) / r.width));
      const ny = Math.min(1, Math.max(0, (clientY - r.top) / r.height));
      onMove(nx, ny);
    },
    [onMove],
  );

  const onPointerDown = (e: React.PointerEvent) => {
    if (disabled) return;
    e.preventDefault();
    dragging.current = true;
    (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
    setFromClient(e.clientX, e.clientY);
  };

  const onPointerMove = (e: React.PointerEvent) => {
    if (!dragging.current || disabled) return;
    setFromClient(e.clientX, e.clientY);
  };

  const onPointerUp = () => {
    dragging.current = false;
  };

  const sample = "12:34:56";
  // Preview frame ~118px wide → height ~66px
  const fontPx = previewFontPx(fontsize, 66);

  return (
    <div className="tc-preview-wrap" aria-hidden>
      <div
        ref={frameRef}
        className={`tc-preview-frame${disabled ? "" : " interactive"}`}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        title="Drag to position"
      >
        <div
          className="tc-preview-overlay free"
          style={{
            left: `${x * 100}%`,
            top: `${y * 100}%`,
            fontSize: `${fontPx}px`,
            opacity,
          }}
        >
          {sample}
        </div>
      </div>
      <div className="tc-preview-snaps" role="group" aria-label="Position presets">
        {SNAPS.map((s) => (
          <button
            key={s.id}
            type="button"
            className="tc-preview-snap"
            disabled={disabled}
            title={s.id.replace("_", " ")}
            onClick={() => {
              onMove(s.x, s.y);
              onSnap?.(s.id);
            }}
          >
            {s.label}
          </button>
        ))}
      </div>
      <span className="tc-preview-caption">Drag to place</span>
    </div>
  );
}
