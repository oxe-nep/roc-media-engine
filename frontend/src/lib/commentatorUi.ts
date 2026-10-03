import type { CommentatorInfo } from "@/lib/api";

export function commentatorIsActive(info?: CommentatorInfo): boolean {
  return !!info?.enabled;
}

export function commentatorPanelClass(info?: CommentatorInfo): string {
  if (!info || !info.enabled) return "stopped";
  if (info.connected) return "running";
  if (info.session_active) return "waiting";
  return "waiting";
}

export function commentatorNumClass(info?: CommentatorInfo): string {
  if (!info || !info.enabled) return "stopped";
  if (info.connected) return "running";
  if (info.session_active) return "waiting";
  return "waiting";
}

export function commentatorStatusLabel(info?: CommentatorInfo): string {
  if (!info || !info.enabled) return "Off";
  if (info.connected) return "Connected";
  if (info.session_active) return "Signaling";
  return "Ready";
}

export function commentatorStatusPillClass(info?: CommentatorInfo): string {
  if (!info || !info.enabled) return "tc-status-pill";
  if (info.connected) return "tc-status-pill running";
  if (info.session_active) return "tc-status-pill starting";
  return "tc-status-pill starting";
}

export function commentatorCardMeta(info?: CommentatorInfo): string {
  if (!info || !info.enabled) return "Off";
  const intercomCount = info.intercom?.filter((s) => s.enabled).length ?? 0;
  const parts: string[] = [];
  if (info.output_format) parts.push(info.output_format);
  if (intercomCount > 0) parts.push(`${intercomCount} intercom`);
  if (info.connected) parts.push("Live");
  else if (info.session_active) parts.push("Signaling");
  else parts.push("No invite");
  return parts.join(" · ");
}

export function commentatorThumbLabel(info?: CommentatorInfo): string {
  if (!info || !info.enabled) return "Off";
  if (info.connected) return "Connected";
  if (info.session_active) return "Waiting for commentator";
  return "Ready";
}
