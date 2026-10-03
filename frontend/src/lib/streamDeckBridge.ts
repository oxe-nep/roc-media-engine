import type { CommentatorIntercomSlot } from "@/lib/api";

export const STREAM_DECK_VOLUME_STEP = 0.05;

export type StreamDeckLayoutButton = {
  slot: number;
  channel: number;
  label: string;
};

export type StreamDeckVolumeAdjust = {
  target: "pgm" | "intercom";
  slot?: number;
  delta: number;
};

export function intercomToLayout(intercom: CommentatorIntercomSlot[]): StreamDeckLayoutButton[] {
  return intercom
    .filter((slot) => slot.enabled)
    .map((slot, index) => ({
      slot: index,
      channel: slot.id,
      label: slot.name?.trim() || `Intercom ${slot.id}`,
    }));
}

export function clampVolume(value: number): number {
  return Math.min(1, Math.max(0, value));
}

export function isIntercomSlotActive(
  layout: StreamDeckLayoutButton[],
  slot: number,
): boolean {
  return layout.some((b) => b.slot === slot);
}
