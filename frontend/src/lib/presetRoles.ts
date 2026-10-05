/** Shared encode-preset helpers for Proxy vs REC editors and channel selectors. */

export function isMezzCodec(codec: string): boolean {
  const c = codec.toLowerCase();
  return c.includes("dnx") || c.includes("xavc");
}

export function isNvencCodec(codec: string): boolean {
  const c = codec.toLowerCase();
  if (isMezzCodec(c)) return false;
  return (
    c.includes("nvenc") ||
    c.includes("nvh264") ||
    c.includes("nvh265") ||
    c.includes("h264") ||
    c.includes("hevc") ||
    c.includes("h265")
  );
}

/** Proxy (= live/SRT) presets: NVENC only. */
export function isProxyPreset(p: { video_codec: string }): boolean {
  return isNvencCodec(p.video_codec);
}

/** HQ / REC file presets: mezz (DNxHD/XAVC) — not the live proxy set. */
export function isRecPreset(p: { video_codec: string }): boolean {
  return !isProxyPreset(p);
}

export type PresetEditorKind = "proxy" | "rec";
