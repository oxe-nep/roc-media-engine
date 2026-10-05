import type { TcLoopInfo, TcLoopPosition, TcLoopSource } from "@/lib/api";

export function defaultTcUdpPort(channelId: number): number {
  return 9300 + channelId;
}

export function tcSourceShort(source?: TcLoopSource): string {
  return source === "external" ? "UDP" : "TOD";
}

/** Compact source label for card status (before timecode). */
export function tcSourceStatusLabel(source?: TcLoopSource): string {
  return source === "external" ? "UDP" : "TOD";
}

/** Card footer status: source prefix + timecode / state. */
export function tcCardStatusMeta(tc?: TcLoopInfo, live?: boolean): string {
  const src = tcSourceStatusLabel(tc?.source);
  const code = normalizeHms(tc?.timecode);
  const hasCode = !!code;

  if (live && hasCode) return `${src} · ${code}`;
  if (hasCode && tcIsActive(tc)) return `${src} · ${code}`;

  if (live) return `${src} · live`;
  if (tc?.status === "restarting") return `${src} · …`;
  if (tc?.status === "error") return `${src} · err`;
  if (tcIsActive(tc)) return `${src} · …`;
  return "Off";
}

/** Keep HH:MM:SS only (drop frames if present). */
function normalizeHms(raw?: string): string | null {
  const s = raw?.trim();
  if (!s || s === "--:--:--") return null;
  const parts = s.split(/[:;.]/);
  if (parts.length < 3) return s;
  const [h, m, sec] = parts;
  if (!/^\d{1,2}$/.test(h) || !/^\d{1,2}$/.test(m) || !/^\d{1,2}$/.test(sec)) return s;
  return `${h.padStart(2, "0")}:${m.padStart(2, "0")}:${sec.padStart(2, "0")}`;
}

export function tcSourceLabel(
  source?: TcLoopSource,
  udpPort?: number,
  channelId?: number,
): string {
  if (source === "external") {
    const port = udpPort && udpPort > 0 ? udpPort : defaultTcUdpPort(channelId ?? 0);
    return `UDP :${port}`;
  }
  return "Time Of Day";
}

export function tcStatusLabel(status?: TcLoopInfo["status"], enabled?: boolean): string {
  if (status === "running") return "On";
  if (status === "restarting") return "…";
  if (status === "error") return "Err";
  if (enabled) return "…";
  return "Off";
}

export function tcStatusPillClass(status?: TcLoopInfo["status"], enabled?: boolean): string {
  if (status === "running") return "tc-status-pill running";
  if (status === "restarting") return "tc-status-pill starting";
  if (status === "error") return "tc-status-pill error";
  if (enabled) return "tc-status-pill starting";
  return "tc-status-pill off";
}

export function tcBadgeText(tc?: TcLoopInfo): string {
  if (!tc || (!tc.enabled && tc.status !== "running" && tc.status !== "restarting")) return "";
  const src = tcSourceShort(tc.source);
  if (tc.status === "running") return `TC · ${src}`;
  if (tc.status === "restarting") return `TC · ${src} · …`;
  if (tc.status === "error") return `TC · ${src} · err`;
  if (tc.enabled) return `TC · ${src} · …`;
  return "";
}

export function tcIsActive(tc?: TcLoopInfo): boolean {
  return !!tc?.enabled || tc?.status === "running" || tc?.status === "restarting";
}

export function tcPreviewHasSignal(tc?: TcLoopInfo): boolean {
  return tc?.status === "running";
}

const POSITION_GRID: Record<TcLoopPosition, string> = {
  top_left: "tl",
  top_right: "tr",
  center: "c",
  bottom_left: "bl",
  bottom_right: "br",
};

export function tcPositionCell(pos: TcLoopPosition): string {
  return POSITION_GRID[pos] ?? "tl";
}
