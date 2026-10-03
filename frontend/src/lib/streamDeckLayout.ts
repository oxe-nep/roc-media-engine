/** Logical key roles for commentator Stream Deck layouts (col/row grid). */

import type { StreamDeckLayoutButton } from "@/lib/streamDeckBridge";

export type StreamDeckKeyRole =
  | { kind: "ptt"; slot: number }
  | { kind: "hosta" }
  | { kind: "pgm_vol"; direction: "up" | "down" }
  | { kind: "ic_vol"; slot: number; direction: "up" | "down" }
  | { kind: "page"; page: 1 | 2 };

export type GridKey = { col: number; row: number; role: StreamDeckKeyRole };

export type StreamDeckGridPreset = {
  id: string;
  label: string;
  columns: number;
  rows: number;
  page1: GridKey[];
  page2: GridKey[];
};

/** Matches former bundled profiles (XL 8×4, Standard 5×3, Mini 3×2). */
export const STREAM_DECK_PRESETS: StreamDeckGridPreset[] = [
  {
    id: "xl",
    label: "Stream Deck XL",
    columns: 8,
    rows: 4,
    page1: [
      { col: 0, row: 0, role: { kind: "ptt", slot: 0 } },
      { col: 1, row: 0, role: { kind: "ptt", slot: 1 } },
      { col: 2, row: 0, role: { kind: "ptt", slot: 2 } },
      { col: 3, row: 0, role: { kind: "ptt", slot: 3 } },
      { col: 4, row: 0, role: { kind: "ptt", slot: 4 } },
      { col: 5, row: 0, role: { kind: "ptt", slot: 5 } },
      { col: 6, row: 0, role: { kind: "pgm_vol", direction: "down" } },
      { col: 7, row: 0, role: { kind: "pgm_vol", direction: "up" } },
      { col: 0, row: 1, role: { kind: "hosta" } },
      { col: 7, row: 1, role: { kind: "page", page: 2 } },
    ],
    page2: [
      { col: 0, row: 0, role: { kind: "ic_vol", slot: 0, direction: "down" } },
      { col: 1, row: 0, role: { kind: "ic_vol", slot: 0, direction: "up" } },
      { col: 2, row: 0, role: { kind: "ic_vol", slot: 1, direction: "down" } },
      { col: 3, row: 0, role: { kind: "ic_vol", slot: 1, direction: "up" } },
      { col: 4, row: 0, role: { kind: "ic_vol", slot: 2, direction: "down" } },
      { col: 5, row: 0, role: { kind: "ic_vol", slot: 2, direction: "up" } },
      { col: 6, row: 0, role: { kind: "ic_vol", slot: 3, direction: "down" } },
      { col: 7, row: 0, role: { kind: "ic_vol", slot: 3, direction: "up" } },
      { col: 0, row: 1, role: { kind: "ic_vol", slot: 4, direction: "down" } },
      { col: 1, row: 1, role: { kind: "ic_vol", slot: 4, direction: "up" } },
      { col: 2, row: 1, role: { kind: "ic_vol", slot: 5, direction: "down" } },
      { col: 3, row: 1, role: { kind: "ic_vol", slot: 5, direction: "up" } },
      { col: 7, row: 1, role: { kind: "page", page: 1 } },
    ],
  },
  {
    id: "standard",
    label: "Stream Deck",
    columns: 5,
    rows: 3,
    page1: [
      { col: 0, row: 0, role: { kind: "ptt", slot: 0 } },
      { col: 1, row: 0, role: { kind: "ptt", slot: 1 } },
      { col: 2, row: 0, role: { kind: "ptt", slot: 2 } },
      { col: 3, row: 0, role: { kind: "ptt", slot: 3 } },
      { col: 4, row: 0, role: { kind: "ptt", slot: 4 } },
      { col: 0, row: 1, role: { kind: "ptt", slot: 5 } },
      { col: 1, row: 1, role: { kind: "pgm_vol", direction: "down" } },
      { col: 2, row: 1, role: { kind: "pgm_vol", direction: "up" } },
      { col: 0, row: 2, role: { kind: "hosta" } },
      { col: 4, row: 1, role: { kind: "page", page: 2 } },
    ],
    page2: [
      { col: 0, row: 0, role: { kind: "ic_vol", slot: 0, direction: "down" } },
      { col: 1, row: 0, role: { kind: "ic_vol", slot: 0, direction: "up" } },
      { col: 2, row: 0, role: { kind: "ic_vol", slot: 1, direction: "down" } },
      { col: 3, row: 0, role: { kind: "ic_vol", slot: 1, direction: "up" } },
      { col: 4, row: 0, role: { kind: "ic_vol", slot: 2, direction: "down" } },
      { col: 0, row: 1, role: { kind: "ic_vol", slot: 2, direction: "up" } },
      { col: 1, row: 1, role: { kind: "ic_vol", slot: 3, direction: "down" } },
      { col: 2, row: 1, role: { kind: "ic_vol", slot: 3, direction: "up" } },
      { col: 3, row: 1, role: { kind: "ic_vol", slot: 4, direction: "down" } },
      { col: 4, row: 1, role: { kind: "ic_vol", slot: 4, direction: "up" } },
      { col: 0, row: 2, role: { kind: "ic_vol", slot: 5, direction: "down" } },
      { col: 1, row: 2, role: { kind: "ic_vol", slot: 5, direction: "up" } },
      { col: 4, row: 2, role: { kind: "page", page: 1 } },
    ],
  },
  {
    id: "mini",
    label: "Stream Deck Mini",
    columns: 3,
    rows: 2,
    page1: [
      { col: 0, row: 0, role: { kind: "ptt", slot: 0 } },
      { col: 1, row: 0, role: { kind: "ptt", slot: 1 } },
      { col: 2, row: 0, role: { kind: "ptt", slot: 2 } },
      { col: 0, row: 1, role: { kind: "ptt", slot: 3 } },
      { col: 1, row: 1, role: { kind: "ptt", slot: 4 } },
      { col: 2, row: 1, role: { kind: "page", page: 2 } },
    ],
    page2: [
      { col: 0, row: 0, role: { kind: "pgm_vol", direction: "down" } },
      { col: 1, row: 0, role: { kind: "pgm_vol", direction: "up" } },
      { col: 2, row: 0, role: { kind: "hosta" } },
      { col: 0, row: 1, role: { kind: "ic_vol", slot: 0, direction: "down" } },
      { col: 1, row: 1, role: { kind: "ic_vol", slot: 0, direction: "up" } },
      { col: 2, row: 1, role: { kind: "page", page: 1 } },
    ],
  },
];

export function detectStreamDeckPreset(columns: number, rows: number): StreamDeckGridPreset {
  const exact = STREAM_DECK_PRESETS.find((p) => p.columns === columns && p.rows === rows);
  if (exact) return exact;
  if (columns >= 8) return STREAM_DECK_PRESETS[0];
  if (columns >= 5) return STREAM_DECK_PRESETS[1];
  return STREAM_DECK_PRESETS[2];
}

export function isRoleRelevant(
  role: StreamDeckKeyRole,
  layout: StreamDeckLayoutButton[],
): boolean {
  switch (role.kind) {
    case "ptt":
    case "ic_vol":
      return layout.some((b) => b.slot === role.slot);
    case "pgm_vol":
    case "hosta":
      return true;
    case "page":
      // Volume page is only useful when at least one intercom is enabled.
      return role.page === 1 || layout.length > 0;
  }
}

export function roleMapForPage(
  preset: StreamDeckGridPreset,
  page: 1 | 2,
  layout: StreamDeckLayoutButton[],
): Map<string, StreamDeckKeyRole> {
  const keys = page === 1 ? preset.page1 : preset.page2;
  const map = new Map<string, StreamDeckKeyRole>();
  for (const { col, row, role } of keys) {
    if (isRoleRelevant(role, layout)) {
      map.set(`${col},${row}`, role);
    }
  }
  return map;
}
