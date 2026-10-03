"use client";

import type { TcLoopPosition } from "@/lib/api";
import { tcPositionCell } from "@/lib/tcUi";

type Props = {
  position: TcLoopPosition;
  fontsize: number;
  opacity: number;
};

const CELLS: { id: TcLoopPosition; label: string }[] = [
  { id: "top_left", label: "TL" },
  { id: "top_right", label: "TR" },
  { id: "center", label: "C" },
  { id: "bottom_left", label: "BL" },
  { id: "bottom_right", label: "BR" },
];

export default function TcPositionPreview({ position, fontsize, opacity }: Props) {
  const active = tcPositionCell(position);
  const sample = "12:34:56";
  const fontScale = Math.max(0.45, Math.min(1, fontsize / 96));

  return (
    <div className="tc-preview-wrap" aria-hidden>
      <div className="tc-preview-frame">
        <div
          className={`tc-preview-overlay ${active}`}
          style={{
            fontSize: `${fontScale * 0.62}rem`,
            opacity,
          }}
        >
          {sample}
        </div>
        <div className="tc-preview-grid">
          {CELLS.map((cell) => (
            <span
              key={cell.id}
              className={`tc-preview-cell ${cell.id.replace("_", "-")}${position === cell.id ? " active" : ""}`}
              title={cell.label}
            />
          ))}
        </div>
      </div>
      <span className="tc-preview-caption">Overlay preview</span>
    </div>
  );
}
