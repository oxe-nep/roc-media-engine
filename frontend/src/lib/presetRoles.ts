/** Shared encode-preset helpers for Proxy vs REC editors and channel selectors. */

export function isProresCodec(codec: string): boolean {
  return codec.toLowerCase().includes("prores");
}

export function isMezzCodec(codec: string): boolean {
  const c = codec.toLowerCase();
  return c.includes("dnx") || isProresCodec(c);
}

/** Display label — ProRes is CPU/NFS-sensitive on current hardware. */
export function formatPresetLabel(p: { id?: string; label: string; video_codec: string }): string {
  const base = (p.label || p.id || "").trim() || "preset";
  if (!isProresCodec(p.video_codec)) return base;
  if (/experimental/i.test(base)) return base;
  return `${base} · experimental`;
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

/** HQ file presets: mezz (DNxHD/ProRes) — not the live proxy set. */
export function isRecPreset(p: { video_codec: string }): boolean {
  return !isProxyPreset(p);
}

export type PresetEditorKind = "proxy" | "rec";
