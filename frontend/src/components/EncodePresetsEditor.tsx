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
import PresetHelpModal from "@/components/PresetHelpModal";

/** Proxy / NVENC average bitrate steps (Mbps). */
const PROXY_MBPS = [3, 4, 5, 6, 8, 10, 12, 15, 20, 25, 30, 40] as const;

/** NVENC mezz file bitrates when used as REC. */
const REC_NVENC_MBPS = [12, 15, 20, 25, 30, 40, 50, 60] as const;

/** Stored nominal Mbps for DNxHD class (live REC overrides from signal). */
const DNXHD_CLASS_MBPS: Record<string, number> = {
  sq: 120,
  hq: 185,
  hqx: 185,
};

/** Nominal Mbps for ProRes profile (encoder is profile-driven). */
const PRORES_PROFILE_MBPS: Record<string, number> = {
  proxy: 45,
  lt: 102,
  standard: 147,
  hq: 220,
};

const PROXY_MBPS_HINT: Record<number, string> = {
  3: "Very light",
  4: "Monitoring",
  6: "Light",
  8: "Web",
  10: "Good",
  12: "Default",
  15: "High",
  20: "Heavy",
  25: "Heavy+",
  30: "Very heavy",
  40: "Max-ish",
};

const DNXHD_CLASS_FALLBACK = [
  { id: "sq", label: "SQ (8-bit)" },
  { id: "hq", label: "HQ (8-bit)" },
  { id: "hqx", label: "HQX (10-bit)" },
];

const PRORES_PROFILE_FALLBACK = [
  { id: "proxy", label: "Proxy" },
  { id: "lt", label: "LT" },
  { id: "standard", label: "422" },
  { id: "hq", label: "HQ" },
];

const NVENC_PRESETS_FALLBACK = [
  { id: "p1", label: "p1 — Fastest" },
  { id: "p2", label: "p2 — Faster" },
  { id: "p3", label: "p3 — Fast" },
  { id: "p4", label: "p4 — Balanced" },
  { id: "p5", label: "p5 — Slow" },
  { id: "p6", label: "p6 — Slower" },
  { id: "p7", label: "p7 — Slowest" },
];

const GOP_OPTIONS = [
  { value: 25, label: "0.5 s" },
  { value: 50, label: "1 s (recommended)" },
  { value: 100, label: "2 s" },
] as const;

const AUDIO_BITRATES = [
  { value: "96k", label: "96 kbps" },
  { value: "128k", label: "128 kbps" },
  { value: "160k", label: "160 kbps" },
  { value: "192k", label: "192 kbps (recommended)" },
  { value: "256k", label: "256 kbps" },
  { value: "320k", label: "320 kbps" },
] as const;

function dnxhdOutcome(cls: string): string {
  switch (cls) {
    case "sq":
      return "1080i50 → DNxHD 120 · 1080p50 → DNxHD 240";
    case "hq":
      return "1080i50 → DNxHD 185 · 1080p50 → DNxHD 365";
    case "hqx":
      return "1080i50 → DNxHD 185x · 1080p50 → DNxHD 365x · needs 10-bit source";
    default:
      return "Bitrate follows live signal (1080i50 / 1080p50)";
  }
}

function normalizeDnxhdClass(preset: string): string {
  const p = preset.toLowerCase();
  if (p === "sq" || p.includes("sq") || p.includes("120") || p.includes("240")) return "sq";
  if (p === "hqx" || p.includes("hqx") || (p.endsWith("x") && p.includes("dnx"))) return "hqx";
  return "hq";
}

function normalizeProresProfile(preset: string): string {
  const p = preset.toLowerCase();
  if (p.includes("proxy")) return "proxy";
  if (p === "lt" || p.includes("_lt") || p.endsWith(" lt")) return "lt";
  if (p === "hq" || p.includes("prores_hq") || p.endsWith("_hq")) return "hq";
  if (p.includes("standard") || p.includes("422")) return "standard";
  return "standard";
}

function dnxhdClassLabel(preset: string): string {
  const cls = normalizeDnxhdClass(preset);
  if (cls === "sq") return "SQ";
  if (cls === "hqx") return "HQX";
  return "HQ";
}

function proresProfileLabel(preset: string): string {
  switch (normalizeProresProfile(preset)) {
    case "proxy":
      return "Proxy";
    case "lt":
      return "LT";
    case "hq":
      return "HQ";
    default:
      return "422";
  }
}

function presetListMeta(p: EncodePreset): string {
  const audio = p.audio_channels === 8 ? "8ch PCM" : "stereo PCM";
  if (p.video_codec.toLowerCase().includes("dnx")) {
    return `${dnxhdClassLabel(p.video_preset)} · ${audio}`;
  }
  if (p.video_codec.toLowerCase().includes("prores")) {
    return `${proresProfileLabel(p.video_preset)} · ${audio}`;
  }
  return `${p.video_bitrate} · ${p.audio_channels === 8 ? "8ch · 4×AAC" : p.audio_bitrate}`;
}

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
  if (c.includes("prores")) return "avenc_prores_ks";
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
      video_preset: "hq",
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
  const c = codec.toLowerCase();
  if (c.includes("dnx") || c.includes("prores")) return [];
  return kind === "proxy" ? PROXY_MBPS : REC_NVENC_MBPS;
}

function pickDefaultPresetId(codec: EncodeCodecOption | undefined, preferred?: string): string {
  const list = codec?.presets ?? [];
  if (preferred && list.some((p) => p.id === preferred)) return preferred;
  const id = codec?.id.toLowerCase() ?? "";
  if (id.includes("dnx")) {
    return list.find((p) => p.id === "hq")?.id ?? list[0]?.id ?? "hq";
  }
  if (id.includes("prores")) {
    return list.find((p) => p.id === "standard" || p.id === "hq")?.id ?? list[0]?.id ?? "standard";
  }
  return (
    list.find((p) => p.id === "p4" || p.id === "hq")?.id ??
    list[0]?.id ??
    preferred ??
    "p4"
  );
}

function mezzNominalMbps(codec: string, profile: string): number {
  const c = codec.toLowerCase();
  if (c.includes("dnx")) return DNXHD_CLASS_MBPS[normalizeDnxhdClass(profile)] ?? 185;
  if (c.includes("prores")) return PRORES_PROFILE_MBPS[normalizeProresProfile(profile)] ?? 147;
  return 185;
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
  const [helpOpen, setHelpOpen] = useState(false);

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
        return {
          ...prev,
          video_codec: first.id,
          video_preset: pickDefaultPresetId(first, prev.video_preset),
        };
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
    setHelpOpen(false);
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
    return presets.filter(isRecPreset).sort((a, b) => a.label.localeCompare(b.label));
  }, [presets, kind]);

  const formMezz = isMezzCodec(form.video_codec);
  const formDnxhd = formMezz && form.video_codec.toLowerCase().includes("dnx");
  const formProres = formMezz && form.video_codec.toLowerCase().includes("prores");
  const dnxClass = formDnxhd ? normalizeDnxhdClass(form.video_preset) : "";
  const proresProfile = formProres ? normalizeProresProfile(form.video_preset) : "";

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
    if (formDnxhd) {
      const list = activeCodec?.presets?.length ? activeCodec.presets : DNXHD_CLASS_FALLBACK;
      if (!list.some((p) => p.id === dnxClass)) {
        return [...list, { id: dnxClass, label: `${dnxClass.toUpperCase()} (current)` }];
      }
      return list;
    }
    if (formProres) {
      const list = activeCodec?.presets?.length ? activeCodec.presets : PRORES_PROFILE_FALLBACK;
      if (!list.some((p) => p.id === proresProfile)) {
        return [...list, { id: proresProfile, label: `${proresProfile} (current)` }];
      }
      return list;
    }
    const list = activeCodec?.presets?.length ? activeCodec.presets : NVENC_PRESETS_FALLBACK;
    if (!list.some((p) => p.id === form.video_preset)) {
      return [...list, { id: form.video_preset, label: `${form.video_preset} (current)` }];
    }
    return list;
  }, [activeCodec, form.video_preset, formDnxhd, formProres, dnxClass, proresProfile]);

  const codecOptions = useMemo(() => {
    if (codecs.some((c) => c.id === form.video_codec) || !form.video_codec) {
      return codecs;
    }
    return [...codecs, { id: form.video_codec, label: `${form.video_codec} (current)`, presets: [] }];
  }, [codecs, form.video_codec]);

  const gopOptions = useMemo(() => {
    if (formMezz) {
      return [{ value: 1, label: "Intra" }];
    }
    const base: { value: number; label: string }[] = [...GOP_OPTIONS];
    if (!base.some((o) => o.value === form.video_gop)) {
      base.push({ value: form.video_gop, label: `Custom (${form.video_gop})` });
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
      base.video_preset = pickDefaultPresetId(codecs[0]);
      if (isMezzCodec(codecs[0].id)) {
        base.video_gop = 1;
        base.audio_channels = 8;
        Object.assign(base, deriveFromMbps(mezzNominalMbps(codecs[0].id, base.video_preset)));
      }
    }
    setForm(base);
  };

  const startEdit = (p: EncodePreset) => {
    setEditingId(p.id);
    const next = { ...p, audio_channels: p.audio_channels === 8 ? 8 : 2 };
    if (p.video_codec.toLowerCase().includes("dnx")) {
      next.video_preset = normalizeDnxhdClass(p.video_preset);
      Object.assign(next, deriveFromMbps(mezzNominalMbps(p.video_codec, next.video_preset)));
    } else if (p.video_codec.toLowerCase().includes("prores")) {
      next.video_preset = normalizeProresProfile(p.video_preset);
      Object.assign(next, deriveFromMbps(mezzNominalMbps(p.video_codec, next.video_preset)));
    }
    setForm(next);
  };

  const setVideoMbps = (mbps: number) => {
    setForm((prev) => ({ ...prev, ...deriveFromMbps(mbps) }));
  };

  const setCodec = (codecId: string) => {
    const codec = codecs.find((c) => c.id === codecId);
    setForm((prev) => {
      const mezz = isMezzCodec(codecId);
      const nextPreset = pickDefaultPresetId(codec, prev.video_preset);
      const next: EncodePreset = {
        ...prev,
        video_codec: codecId,
        video_preset: nextPreset,
      };
      if (mezz) {
        next.video_gop = 1;
        next.audio_channels = 8;
        Object.assign(next, deriveFromMbps(mezzNominalMbps(codecId, nextPreset)));
      } else if (isMezzCodec(prev.video_codec)) {
        next.video_gop = 50;
        next.audio_channels = kind === "rec" ? 2 : prev.audio_channels;
        Object.assign(next, deriveFromMbps(kind === "proxy" ? 12 : 20));
      }
      return next;
    });
  };

  const setDnxhdClass = (cls: string) => {
    const id = normalizeDnxhdClass(cls);
    setForm((prev) => ({
      ...prev,
      video_preset: id,
      ...deriveFromMbps(DNXHD_CLASS_MBPS[id] ?? 185),
    }));
  };

  const setProresProfile = (profile: string) => {
    const id = normalizeProresProfile(profile);
    setForm((prev) => ({
      ...prev,
      video_preset: id,
      ...deriveFromMbps(PRORES_PROFILE_MBPS[id] ?? 147),
    }));
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
      const mbps = Math.round(
        parseMbps(form.video_bitrate) ??
          (formDnxhd
            ? DNXHD_CLASS_MBPS[dnxClass] ?? 185
            : formProres
              ? PRORES_PROFILE_MBPS[proresProfile] ?? 147
              : formMezz
                ? 185
                : 12),
      );
      const payload: EncodePreset = {
        ...form,
        ...deriveFromMbps(mbps),
        video_codec: normalizeCodecId(form.video_codec),
        video_preset: formDnxhd ? dnxClass : formProres ? proresProfile : form.video_preset,
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

      <div className="presets-intro-row">
        <p className="settings-tab-intro">
          {kind === "proxy"
            ? "Live / SRT / preview encode. Selected per channel as Proxy preset."
            : "Recording file (mezz). Selected per channel as HQ preset."}
        </p>
        <button type="button" className="badge files-btn" onClick={() => setHelpOpen(true)}>
          Help
        </button>
      </div>

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
                  {p.id} · {presetListMeta(p)}
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
            {editingId ? `Edit: ${form.label || editingId}` : `New ${kind === "proxy" ? "proxy" : "HQ"} preset`}
          </div>

          <label className="presets-field">
            <span>Name</span>
            <input
              value={form.label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder={kind === "proxy" ? "e.g. Proxy 12M" : "e.g. ProRes HQ"}
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

            {formDnxhd ? (
              <label className="presets-field">
                <span>DNxHD class</span>
                <select
                  value={dnxClass}
                  onChange={(e) => setDnxhdClass(e.target.value)}
                  disabled={busy}
                >
                  {encoderPresets.map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.label}
                    </option>
                  ))}
                </select>
              </label>
            ) : formProres ? (
              <label className="presets-field">
                <span>ProRes profile</span>
                <select
                  value={proresProfile}
                  onChange={(e) => setProresProfile(e.target.value)}
                  disabled={busy}
                >
                  {encoderPresets.map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.label}
                    </option>
                  ))}
                </select>
              </label>
            ) : (
              <>
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
                  <span>Encoder speed</span>
                  <select
                    value={form.video_preset}
                    onChange={(e) => setForm((prev) => ({ ...prev, video_preset: e.target.value }))}
                    disabled={busy}
                  >
                    {encoderPresets.map((p) => (
                      <option key={p.id} value={p.id}>
                        {p.label}
                      </option>
                    ))}
                  </select>
                </label>
              </>
            )}

            {!formMezz && (
              <label className="presets-field">
                <span>Keyframe interval</span>
                <select
                  value={form.video_gop}
                  onChange={(e) =>
                    setForm((prev) => ({ ...prev, video_gop: Number(e.target.value) || 50 }))
                  }
                  disabled={busy}
                >
                  {gopOptions.map((g) => (
                    <option key={g.value} value={g.value}>
                      {g.label}
                    </option>
                  ))}
                </select>
              </label>
            )}

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
                    <option value="2">Stereo PCM</option>
                    <option value="8">8ch PCM (4× stereo)</option>
                  </>
                ) : (
                  <>
                    <option value="2">Stereo AAC</option>
                    <option value="8">8ch AAC (4× stereo)</option>
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
            {formDnxhd ? (
              <>
                MXF · DNxHD {dnxhdClassLabel(dnxClass)} · {dnxhdOutcome(dnxClass)} · {audioMeta}
              </>
            ) : formProres ? (
              <>
                MOV · ProRes {proresProfileLabel(proresProfile)} · progressive from live ·{" "}
                {audioMeta}
              </>
            ) : (
              <>
                MP4 · {form.video_codec} · {derived.video_bitrate}
                {` (max ${derived.video_maxrate})`} · {form.video_preset} · {audioMeta}
              </>
            )}
          </p>

          <div className="presets-form-actions">
            <button type="button" className="global-rec-btn" onClick={save} disabled={busy}>
              {busy ? "…" : editingId ? "Save changes" : "Create preset"}
            </button>
          </div>
        </div>
      </div>

      <PresetHelpModal open={helpOpen} kind={kind} onClose={() => setHelpOpen(false)} />
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
