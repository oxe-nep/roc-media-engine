export type ChannelWorkflowMode = "pair" | "tc" | "remote_commentator";

export interface ChannelWorkflowConfig {
  mode: ChannelWorkflowMode;
}

export const DEFAULT_WORKFLOW: ChannelWorkflowConfig = { mode: "pair" };

/** Modes shown in Settings — commentator is hidden until the engine bridge exists. */
export const WORKFLOW_OPTIONS: {
  mode: ChannelWorkflowMode;
  label: string;
  hint: string;
}[] = [
  { mode: "pair", label: "Encode + Decode", hint: "Record / ingest and playout" },
  { mode: "tc", label: "TC Burn-In", hint: "Timecode overlay loop" },
];

const VALID_MODES = new Set<ChannelWorkflowMode>(["pair", "tc"]);

export function normalizeWorkflowConfig(
  value?: Partial<ChannelWorkflowConfig> | { encode?: boolean; decode?: boolean } | string,
): ChannelWorkflowConfig {
  if (typeof value === "string") {
    if (value === "tc") return { mode: "tc" };
    // Legacy saved mode — treat as pair until commentator ships.
    if (value === "remote_commentator") return { ...DEFAULT_WORKFLOW };
    return { ...DEFAULT_WORKFLOW };
  }
  if (value && "mode" in value && value.mode && VALID_MODES.has(value.mode)) {
    return { mode: value.mode };
  }
  if (value && "mode" in value && value.mode === "remote_commentator") {
    return { ...DEFAULT_WORKFLOW };
  }
  // Transitional encode/decode booleans from older builds.
  if (value && ("encode" in value || "decode" in value)) {
    return { ...DEFAULT_WORKFLOW };
  }
  return { ...DEFAULT_WORKFLOW };
}

export function workflowForChannel(
  workflows: Record<number, ChannelWorkflowConfig>,
  channelId: number,
): ChannelWorkflowConfig {
  return normalizeWorkflowConfig(workflows[channelId]);
}

export function workflowMode(
  workflows: Record<number, ChannelWorkflowConfig>,
  channelId: number,
): ChannelWorkflowMode {
  return workflowForChannel(workflows, channelId).mode;
}

export function showEncodeCard(
  workflows: Record<number, ChannelWorkflowConfig>,
  channelId: number,
): boolean {
  return workflowMode(workflows, channelId) === "pair";
}

export function showDecodeCard(
  workflows: Record<number, ChannelWorkflowConfig>,
  channelId: number,
): boolean {
  return workflowMode(workflows, channelId) === "pair";
}

export function showCommentatorCard(
  workflows: Record<number, ChannelWorkflowConfig>,
  channelId: number,
): boolean {
  return workflowMode(workflows, channelId) === "remote_commentator";
}

export function showTcCard(
  workflows: Record<number, ChannelWorkflowConfig>,
  channelId: number,
): boolean {
  return workflowMode(workflows, channelId) === "tc";
}

export function isTcWorkflow(
  workflows: Record<number, ChannelWorkflowConfig>,
  channelId: number,
): boolean {
  return workflowMode(workflows, channelId) === "tc";
}
