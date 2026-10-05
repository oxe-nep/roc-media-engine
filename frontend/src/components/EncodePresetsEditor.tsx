"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  createEncodePreset,
  deleteEncodePreset,
  fetchEncodeOptions,
  fetchEncodePresets,
  updateEncodePreset,
  type EncodeCodecOption,
  type EncodePreset,
} from "@/lib/api";
import { useBodyScrollLock } from "@/hooks/useBodyScrollLock";
import {
  isMezzCodec,
  isNvencCodec,
  isProxyPreset,
  isRecPreset,
  type PresetEditorKind,
} from "@/lib/presetRoles";

/** Proxy / NVENC average bitrate steps (Mbps). */
const PROXY_MBPS = [3, 4, 5, 6, 8, 10, 12, 15, 20, 25, 30, 40] as const;

/** NVENC mezz file bitrates when used as REC. */
const REC_NVENC_MBPS = [12, 15, 20, 25, 30, 40, 50, 60] as const;

/** Valid-ish DNxHD Mbps for 1080p50 (host-probed set). */
const DNXHD_MBPS = [36, 45, 60, 75, 90, 110, 115, 120, 145, 175, 185, 220] as const;

/** XAVC Intra HD approximation bitrate steps. */
const XAVC_MBPS = [50, 75, 100, 111, 140, 160, 200] as const;

const PROXY_MBPS_HINT: Record<number, string> = {
  3: "Very light proxy",
  4: "Proxy / monitoring",
  6: "Light edit",
  8: "Web / review",
  10: "Good quality",
  12: "HQ (default)",
  15: "High quality",
  20: "Heavy proxy",
  25: "Contribution-ish",
  30: "Heavy",
  40: "Near contribution",
};

const NVENC_PRESETS_FALLBACK = [
  { id: "p1", label: "p1 — Fastest (lower quality)" },
  { id: "p2", label: "p2 — Faster" },
  { id: "p3", label: "p3 — Fast" },
  { id: "p4", label: "p4 — Balanced (recommended)" },
  { id: "p5", label: "p5 — Slow" },
  { id: "p6", label: "p6 — Slower" },
  { id: "p7", label: "p7 — Slowest (best quality)" },
];

const GOP_OPTIONS = [
  { value: 25, label: "0.5 s  (GOP 25 @ 50 fps)" },
  { value: 50, label: "1 s  (GOP 50 @ 50 fps) — recommended" },
  { value: 100, label: "2 s  (GOP 100 @ 50 fps)" },
] as const;

const AUDIO_BITRATES = [
  { value: "96k", label: "96 kbps — voice / light" },
  { value: "128k", label: "128 kbps — stereo OK" },
  { value: "160k", label: "160 kbps" },
  { value: "192k", label: "192 kbps — recommended" },
  { value: "256k", label: "256 kbps — high" },
  { value: "320k", label: "320 kbps — max AAC" },
] as const;

function parseMbps(raw: string): number | null {
  const s = raw.trim().toLowerCase();
  const m = s.match(/^([\d.]+)\s*([km])?$/);
  if (!m) return null;
  const n = Number(m[1]);
  if (!Number.isFinite(n) || n <= 0) return null;
  if (m[2] === "k") return n / 1000;
  return n;
}

function deriveFromMbps(mbps: number): Pick<EncodePreset, "video_bitrate" | "video_maxrate" | "video_bufsize"> {
  const br = Math.max(1, Math.round(mbps));
  const max = Math.max(br + 1, Math.round(br * 1.2));
  const buf = Math.max(max + 1, Math.round(br * 2));
  return {
    video_bitrate: `${br}M`,
    video_maxrate: `${max}M`,
    video_bufsize: `${buf}M`,
  };
}

function slugifyId(label: string): string {
  return label
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "")
    .slice(0, 32);
}

function normalizeCodecId(id: string): string {
  const c = id.toLowerCase();
  if (c.includes("dnx")) return "avenc_dnxhd";
  if (c.includes("xavc")) return "xavc_intra";
  if (c.includes("265") || c.includes("hevc")) return "hevc_nvenc";
  return "h264_nvenc";
}

function emptyForm(kind: PresetEditorKind): EncodePreset {
  if (kind === "rec") {
    return {
      id: "",
      label: "",
      video_codec: "avenc_dnxhd",
      ...deriveFromMbps(185),
      video_preset: "dnxhd",
      video_gop: 1,
      audio_bitrate: "384k",
      audio_channels: 8,
    };
  }
  return {
    id: "",
    label: "",
    video_codec: "h264_nvenc",
    ...deriveFromMbps(12),
    video_preset: "p4",
    video_gop: 50,
    audio_bitrate: "192k",
    audio_channels: 2,
  };
}

function filterCodecs(codecs: EncodeCodecOption[], kind: PresetEditorKind): EncodeCodecOption[] {
  if (kind === "proxy") {
    return codecs.filter((c) => isNvencCodec(c.id));
  }
  return codecs;
}

function bitrateStepsFor(codec: string, kind: PresetEditorKind): readonly number[] {
  if (isMezzCodec(codec)) {
    if (codec.toLowerCase().includes("dnx")) return DNXHD_MBPS;
    return XAVC_MBPS;
  }
  return kind === "proxy" ? PROXY_MBPS : REC_NVENC_MBPS;
}

type Props = {
  open: boolean;
  kind: PresetEditorKind;
  onClose?: () => void;
  onChanged?: () => void;
  /** Render inside Settings tab — no backdrop or modal chrome. */
  embedded?: boolean;
};

export default function EncodePresetsEditor({
  open,
  kind,
  onClose,
  onChanged,
  embedded,
}: Props) {
  const [presets, setPresets] = useState<EncodePreset[]>([]);
  const [codecs, setCodecs] = useState<EncodeCodecOption[]>([]);
  const [form, setForm] = useState<EncodePreset>(() => emptyForm(kind));
  const [editingId, setEditingId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useBodyScrollLock(open && !embedded);

  const load = useCallback(async () => {
    try {
      const [list, opts] = await Promise.all([fetchEncodePresets(), fetchEncodeOptions()]);
      const scopedCodecs = filterCodecs(opts, kind);
      setPresets(list);
      setCodecs(scopedCodecs);
      setError(null);
      setForm((prev) => {
        if (scopedCodecs.length === 0) return prev;
        const hasCodec = scopedCodecs.some((c) => c.id === prev.video_codec);
        if (hasCodec) return prev;
        const first = scopedCodecs[0];
        const preset =
          first.presets.find((p) => p.id === prev.video_preset)?.id ??
          first.presets.find((p) => p.id === "p4")?.id ??
          first.presets[0]?.id ??
          prev.video_preset;
        return { ...prev, video_codec: first.id, video_preset: preset };
      });
    } catch (e) {
      setError(String(e));
    }
  }, [kind]);

  useEffect(() => {
    if (!open) return;
    load();
    setEditingId(null);
    setForm(emptyForm(kind));
  }, [open, load, kind]);

  useEffect(() => {
    if (!open || embedded) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose?.();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose, embedded]);

  const visiblePresets = useMemo(() => {
    if (kind === "proxy") return presets.filter(isProxyPreset);
    // REC editor: mezz / HQ file presets only (no live proxy NVENC set).
    return presets
      .filter(isRecPreset)
      .sort((a, b) => a.label.localeCompare(b.label));
  }, [presets, kind]);

  const formMezz = isMezzCodec(form.video_codec);

  const videoMbps = useMemo(() => {
    const n = parseMbps(form.video_bitrate);
    return n ?? (formMezz ? 185 : 12);
  }, [form.video_bitrate, formMezz]);

  const mbpsOptions = useMemo(() => {
    const steps = bitrateStepsFor(form.video_codec, kind);
    const set = new Set<number>([...steps]);
    const cur = Math.round(videoMbps);
    if (cur > 0) set.add(cur);
    return Array.from(set).sort((a, b) => a - b);
  }, [form.video_codec, kind, videoMbps]);

  const activeCodec = useMemo(() => {
    return codecs.find((c) => c.id === form.video_codec) ?? codecs[0] ?? null;
  }, [codecs, form.video_codec]);

  const encoderPresets = useMemo(() => {
    const list = activeCodec?.presets?.length ? activeCodec.presets : NVENC_PRESETS_FALLBACK;
    if (!list.some((p) => p.id === form.video_preset)) {
      return [...list, { id: form.video_preset, label: `${form.video_preset} (current)` }];
    }
    return list;
  }, [activeCodec, form.video_preset]);

  const codecOptions = useMemo(() => {
    if (codecs.some((c) => c.id === form.video_codec) || !form.video_codec) {
      return codecs;
    }
    return [...codecs, { id: form.video_codec, label: `${form.video_codec} (current)`, presets: [] }];
  }, [codecs, form.video_codec]);

  const gopOptions = useMemo(() => {
    if (formMezz) {
      return [{ value: 1, label: "Intra (GOP 1)" }];
    }
    const base: { value: number; label: string }[] = [...GOP_OPTIONS];
    if (!base.some((o) => o.value === form.video_gop)) {
      base.push({ value: form.video_gop, label: `Custom (GOP ${form.video_gop})` });
    }
    return base;
  }, [form.video_gop, formMezz]);

  const audioOptions = useMemo(() => {
    if (AUDIO_BITRATES.some((a) => a.value === form.audio_bitrate)) {
      return [...AUDIO_BITRATES];
    }
    return [
      ...AUDIO_BITRATES,
      { value: form.audio_bitrate, label: `Custom (${form.audio_bitrate})` },
    ];
  }, [form.audio_bitrate]);

  if (!open) return null;

  const startCreate = () => {
    setEditingId(null);
    const base = emptyForm(kind);
    if (codecs[0]) {
      base.video_codec = codecs[0].id;
      base.video_preset =
        codecs[0].presets.find((p) => p.id === "p4" || p.id === "dnxhd" || p.id === "intra")?.id ??
        codecs[0].presets[0]?.id ??
        base.video_preset;
      if (isMezzCodec(codecs[0].id)) {
        base.video_gop = 1;
        base.audio_channels = 8;
        Object.assign(base, deriveFromMbps(codecs[0].id.includes("xavc") ? 111 : 185));
      }
    }
    setForm(base);
  };

  const startEdit = (p: EncodePreset) => {
    setEditingId(p.id);
    setForm({ ...p, audio_channels: p.audio_channels === 8 ? 8 : 2 });
  };

  const setVideoMbps = (mbps: number) => {
    setForm((prev) => ({ ...prev, ...deriveFromMbps(mbps) }));
  };

  const setCodec = (codecId: string) => {
    const codec = codecs.find((c) => c.id === codecId);
    setForm((prev) => {
      const mezz = isMezzCodec(codecId);
      const nextPreset =
        codec?.presets.find((p) => p.id === prev.video_preset)?.id ??
        codec?.presets.find((p) => p.id === "p4" || p.id === "dnxhd" || p.id === "intra")?.id ??
        codec?.presets[0]?.id ??
        prev.video_preset;
      const next: EncodePreset = {
        ...prev,
        video_codec: codecId,
        video_preset: nextPreset,
      };
      if (mezz) {
        next.video_gop = 1;
        next.audio_channels = 8;
        const def = codecId.toLowerCase().includes("xavc") ? 111 : 185;
        Object.assign(next, deriveFromMbps(def));
      } else if (isMezzCodec(prev.video_codec)) {
        next.video_gop = 50;
        next.audio_channels = kind === "rec" ? 2 : prev.audio_channels;
        Object.assign(next, deriveFromMbps(kind === "proxy" ? 12 : 20));
      }
      return next;
    });
  };

  const setLabel = (label: string) => {
    setForm((prev) => {
      const next = { ...prev, label };
      if (!editingId) {
        const slug = slugifyId(label);
        if (slug) next.id = slug;
      }
      return next;
    });
  };

  const save = async () => {
    setBusy(true);
    try {
      const mbps = Math.round(parseMbps(form.video_bitrate) ?? (formMezz ? 185 : 12));
      const payload: EncodePreset = {
        ...form,
        ...deriveFromMbps(mbps),
        video_codec: normalizeCodecId(form.video_codec),
        video_gop: formMezz ? 1 : form.video_gop,
        audio_channels: form.audio_channels === 8 ? 8 : 2,
      };
      if (kind === "proxy" && isMezzCodec(payload.video_codec)) {
        throw new Error("Mezz codecs belong in HQ presets");
      }
      if (editingId) {
        const { id: _id, ...rest } = payload;
        await updateEncodePreset(editingId, rest);
      } else {
        if (!payload.id.trim()) throw new Error("Preset id is required — enter a label first");
        await createEncodePreset(payload);
      }
      await load();
      onChanged?.();
      startCreate();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (id: string) => {
    if (!window.confirm(`Delete preset "${id}"? Channels using it will fall back to default.`)) return;
    setBusy(true);
    try {
      await deleteEncodePreset(id);
      await load();
      onChanged?.();
      if (editingId === id) startCreate();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const derived = deriveFromMbps(Math.round(videoMbps));
  const title = kind === "proxy" ? "Proxy presets" : "HQ presets";
  const audioMeta = formMezz
    ? form.audio_channels === 8
      ? "8ch PCM"
      : "stereo PCM"
    : form.audio_channels === 8
      ? `4× AAC ${form.audio_bitrate}`
      : `AAC stereo ${form.audio_bitrate}`;

  const panel = (
    <>
      {error && <div className="error-message">{error}</div>}

      <p className="settings-tab-intro">
        {kind === "proxy"
          ? "Live / SRT / preview encode (NVENC). Selected per channel as Proxy preset."
          : "Recording encode (NVENC mezz, DNxHD, or XAVC). Selected per channel as HQ preset. Mezz writes MXF with PCM."}
      </p>

      <div className="presets-layout">
        <div className="presets-list">
          <button type="button" className="badge files-btn" onClick={startCreate} disabled={busy}>
            + New
          </button>
          {visiblePresets.map((p) => (
            <div key={p.id} className={`presets-row ${editingId === p.id ? "active" : ""}`}>
              <button type="button" className="presets-row-main" onClick={() => startEdit(p)}>
                <span className="presets-row-label">{p.label}</span>
                <span className="presets-row-meta">
                  {p.id} · {p.video_bitrate} ·{" "}
                  {isMezzCodec(p.video_codec)
                    ? p.audio_channels === 8
                      ? "8ch PCM"
                      : "stereo PCM"
                    : p.audio_channels === 8
                      ? "8ch · 4×AAC"
                      : p.audio_bitrate}
                </span>
              </button>
              <button type="button" className="badge delete-btn" onClick={() => remove(p.id)} disabled={busy}>
                Delete
              </button>
            </div>
          ))}
        </div>

        <div className="presets-form">
          <div className="presets-form-title">
            {editingId ? `Edit: ${form.label || editingId}` : `New ${kind === "proxy" ? "proxy" : "REC"} preset`}
          </div>

          <label className="presets-field">
            <span>Name</span>
            <input
              value={form.label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder={kind === "proxy" ? "e.g. HQ 12 Mbit" : "e.g. DNxHD 185"}
              disabled={busy}
            />
          </label>
          {!editingId && (
            <label className="presets-field">
              <span>Internal ID</span>
              <input
                value={form.id}
                onChange={(e) => setForm((prev) => ({ ...prev, id: slugifyId(e.target.value) || e.target.value }))}
                placeholder="auto from name"
                disabled={busy}
              />
            </label>
          )}

          <div className="presets-grid">
            <label className="presets-field">
              <span>Video codec</span>
              <select
                value={form.video_codec}
                onChange={(e) => setCodec(e.target.value)}
                disabled={busy || codecOptions.length === 0}
              >
                {codecOptions.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.label}
                  </option>
                ))}
              </select>
            </label>

            <label className="presets-field">
              <span>Video bitrate</span>
              <select
                value={String(Math.round(videoMbps))}
                onChange={(e) => setVideoMbps(Number(e.target.value))}
                disabled={busy}
              >
                {mbpsOptions.map((m) => (
                  <option key={m} value={m}>
                    {m} Mbps
                    {!formMezz && PROXY_MBPS_HINT[m] ? ` — ${PROXY_MBPS_HINT[m]}` : ""}
                  </option>
                ))}
              </select>
            </label>

            <label className="presets-field">
              <span>{formMezz ? "Profile" : "Encoder speed / quality"}</span>
              <select
                value={form.video_preset}
                onChange={(e) => setForm((prev) => ({ ...prev, video_preset: e.target.value }))}
                disabled={busy || formMezz}
              >
                {encoderPresets.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.label}
                  </option>
                ))}
              </select>
            </label>

            <label className="presets-field">
              <span>Keyframe interval</span>
              <select
                value={formMezz ? 1 : form.video_gop}
                onChange={(e) =>
                  setForm((prev) => ({ ...prev, video_gop: Number(e.target.value) || 50 }))
                }
                disabled={busy || formMezz}
              >
                {gopOptions.map((g) => (
                  <option key={g.value} value={g.value}>
                    {g.label}
                  </option>
                ))}
              </select>
            </label>

            <label className="presets-field">
              <span>Audio</span>
              <select
                value={String(form.audio_channels === 8 ? 8 : 2)}
                onChange={(e) =>
                  setForm((prev) => ({ ...prev, audio_channels: Number(e.target.value) === 8 ? 8 : 2 }))
                }
                disabled={busy}
              >
                {formMezz ? (
                  <>
                    <option value="2">Stereo — PCM 24-bit</option>
                    <option value="8">8 tracks — 4× PCM stereo pairs</option>
                  </>
                ) : (
                  <>
                    <option value="2">Stereo — AAC</option>
                    <option value="8">8 tracks — 4× AAC stereo pairs</option>
                  </>
                )}
              </select>
            </label>

            {!formMezz && (
              <label className="presets-field">
                <span>Audio bitrate</span>
                <select
                  value={form.audio_bitrate}
                  onChange={(e) => setForm((prev) => ({ ...prev, audio_bitrate: e.target.value }))}
                  disabled={busy}
                >
                  {audioOptions.map((a) => (
                    <option key={a.value} value={a.value}>
                      {a.label}
                    </option>
                  ))}
                </select>
              </label>
            )}
          </div>

          <p className="presets-derived">
            {formMezz ? "MXF" : "MP4"} · {form.video_codec} · {derived.video_bitrate}
            {!formMezz ? ` (max ${derived.video_maxrate}, buffer ${derived.video_bufsize})` : ""} ·{" "}
            {form.video_preset} · GOP {formMezz ? 1 : form.video_gop} · {audioMeta}
          </p>

          <p className="presets-hint">
            {kind === "proxy"
              ? "Saving restarts capture on channels that use this as Proxy preset (blocked while recording)."
              : "Mezz changes apply on the next HQ start. NVENC HQ presets relaunch channels that use them for live encode."}
          </p>
          <div className="presets-form-actions">
            <button type="button" className="global-rec-btn" onClick={save} disabled={busy}>
              {busy ? "…" : editingId ? "Save changes" : "Create preset"}
            </button>
          </div>
        </div>
      </div>
    </>
  );

  if (embedded) {
    return <div className="settings-tab-panel">{panel}</div>;
  }

  return (
    <div className="modal-backdrop" onClick={onClose} role="presentation">
      <div
        className="modal-panel presets-modal"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-label={title}
      >
        <div className="modal-header">
          <h2>{title}</h2>
          <button type="button" className="modal-close" onClick={onClose} aria-label="Close">
            ×
          </button>
        </div>
        {panel}
      </div>
    </div>
  );
}
