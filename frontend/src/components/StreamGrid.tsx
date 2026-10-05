"use client";

import { useEffect, useRef, useState } from "react";
import {
  fetchEncodePresets,
  fetchLibraryCategories,
  setRecordingCategory,
  setRecordingName,
  startProxyRecording,
  stopProxyRecording,
  startHqRecording,
  stopHqRecording,
  startSrt,
  stopSrt,
  type EncodePreset,
  type LibraryCategory,
  type RecordingSchedule,
  isCaptureOn,
} from "@/lib/api";
import { showEncodeCard } from "@/lib/workflow";
import { sortByChannelId } from "@/lib/sortChannels";
import { useWorkflows } from "@/hooks/useWorkflows";
import { useDashboard } from "@/hooks/useDashboard";
import Thumbnail from "@/components/Thumbnail";
import PreviewModal from "@/components/PreviewModal";
import AudioMeters from "@/components/AudioMeters";
import ListenButton from "@/components/ListenButton";
import ChannelSettingsModal from "@/components/ChannelSettingsModal";

function formatElapsed(sec?: number): string {
  if (sec === undefined || Number.isNaN(sec) || sec < 0) return "00:00:00";
  const s = Math.floor(sec);
  const hh = Math.floor(s / 3600);
  const mm = Math.floor((s % 3600) / 60);
  const ss = s % 60;
  return [hh, mm, ss].map((n) => String(n).padStart(2, "0")).join(":");
}

function formatBitrate(kbps?: number): string {
  if (!kbps || kbps <= 0) return "--";
  if (kbps >= 1000) return `${(kbps / 1000).toFixed(1)} Mbit/s`;
  return `${kbps.toFixed(0)} kbit/s`;
}

/** Prefer short mode labels: "1920x1080p50/1 (1080p50)" → "1080p50". */
function shortSignalFormat(raw?: string): string | null {
  if (!raw) return null;
  const paren = raw.match(/\(([^)]+)\)\s*$/);
  if (paren?.[1]) return paren[1].trim();
  return raw;
}

function formatSchedBadge(sch: RecordingSchedule): string {
  const hm = (iso: string) => {
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return "--:--";
    return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
  };
  const range = `${hm(sch.start_at)}–${hm(sch.stop_at)}`;
  const arms: string[] = [];
  if (sch.arm_proxy) arms.push("PROXY");
  if (sch.arm_hq ?? true) arms.push("HQ");
  const armLabel = arms.length ? arms.join("+") : "HQ";
  if (sch.phase === "waiting") return `SCHED ${armLabel} wait · ${range}`;
  return `SCHED ${armLabel} · ${range}`;
}

export default function StreamGrid() {
  const { loading, streams, recordings, srtById } = useDashboard();
  const [presets, setPresets] = useState<EncodePreset[]>([]);
  const [categories, setCategories] = useState<LibraryCategory[]>([]);
  const [cardError, setCardError] = useState<Record<number, string>>({});
  const [proxyRecBusy, setProxyRecBusy] = useState<Record<number, boolean>>({});
  const [hqRecBusy, setHqRecBusy] = useState<Record<number, boolean>>({});
  const [srtBusy, setSrtBusy] = useState<Record<number, boolean>>({});
  const [preview, setPreview] = useState<{ id: number; pair: number } | null>(null);
  const [settingsId, setSettingsId] = useState<number | null>(null);
  const [editingNameId, setEditingNameId] = useState<number | null>(null);
  const [nameDraft, setNameDraft] = useState("");
  const [metaBusy, setMetaBusy] = useState<Record<number, boolean>>({});
  const nameInputRef = useRef<HTMLInputElement>(null);
  const { workflows } = useWorkflows();

  useEffect(() => {
    if (editingNameId == null) return;
    nameInputRef.current?.focus();
    nameInputRef.current?.select();
  }, [editingNameId]);

  useEffect(() => {
    const refreshPresets = () => {
      fetchEncodePresets()
        .then(setPresets)
        .catch(() => {});
    };
    const refreshCategories = () => {
      fetchLibraryCategories()
        .then(setCategories)
        .catch(() => {});
    };
    fetchEncodePresets()
      .then(setPresets)
      .catch(() => {});
    refreshCategories();
    window.addEventListener("roc-presets-changed", refreshPresets);
    window.addEventListener("roc-library-changed", refreshCategories);
    return () => {
      window.removeEventListener("roc-presets-changed", refreshPresets);
      window.removeEventListener("roc-library-changed", refreshCategories);
    };
  }, []);

  const clearCardError = (id: number) => {
    setCardError((prev) => {
      if (!prev[id]) return prev;
      const next = { ...prev };
      delete next[id];
      return next;
    });
  };

  const toggleProxyRecording = async (id: number) => {
    const recording = recordings[id]?.proxy?.status === "recording";
    if (recording) {
      if (!window.confirm(`Stop proxy recording on channel ${id}?`)) return;
    }
    setProxyRecBusy((b) => ({ ...b, [id]: true }));
    clearCardError(id);
    try {
      if (recording) await stopProxyRecording(id);
      else await startProxyRecording(id);
    } catch (e) {
      setCardError((prev) => ({ ...prev, [id]: String(e) }));
    } finally {
      setProxyRecBusy((b) => ({ ...b, [id]: false }));
    }
  };

  const toggleHqRecording = async (id: number) => {
    const recording =
      recordings[id]?.hq?.status === "recording" ||
      (recordings[id]?.hq == null &&
        recordings[id]?.proxy == null &&
        recordings[id]?.status === "recording");
    if (recording) {
      if (!window.confirm(`Stop HQ recording on channel ${id}?`)) return;
    }
    setHqRecBusy((b) => ({ ...b, [id]: true }));
    clearCardError(id);
    try {
      if (recording) await stopHqRecording(id);
      else await startHqRecording(id);
    } catch (e) {
      setCardError((prev) => ({ ...prev, [id]: String(e) }));
    } finally {
      setHqRecBusy((b) => ({ ...b, [id]: false }));
    }
  };

  const toggleSrt = async (id: number) => {
    setSrtBusy((b) => ({ ...b, [id]: true }));
    clearCardError(id);
    try {
      if (srtById[id]?.status === "streaming") await stopSrt(id);
      else await startSrt(id);
    } catch (e) {
      setCardError((prev) => ({ ...prev, [id]: String(e) }));
    } finally {
      setSrtBusy((b) => ({ ...b, [id]: false }));
    }
  };

  const beginEditName = (id: number, current: string) => {
    setEditingNameId(id);
    setNameDraft(current);
  };

  const commitName = async (id: number) => {
    const clean = nameDraft.trim() || `ch${id}`;
    const prev = recordings[id]?.name || `ch${id}`;
    setEditingNameId(null);
    if (clean === prev) return;
    setMetaBusy((b) => ({ ...b, [id]: true }));
    clearCardError(id);
    try {
      await setRecordingName(id, clean);
    } catch (e) {
      setCardError((prev) => ({ ...prev, [id]: String(e) }));
    } finally {
      setMetaBusy((b) => ({ ...b, [id]: false }));
    }
  };

  const changeCategory = async (id: number, next: string) => {
    const prev = recordings[id]?.category || "_unsorted";
    if (!next || next === prev) return;
    setMetaBusy((b) => ({ ...b, [id]: true }));
    clearCardError(id);
    try {
      await setRecordingCategory(id, next);
    } catch (e) {
      setCardError((prev) => ({ ...prev, [id]: String(e) }));
    } finally {
      setMetaBusy((b) => ({ ...b, [id]: false }));
    }
  };

  const anyRecording = Object.values(recordings).some((r) => r.status === "recording");

  useEffect(() => {
    window.dispatchEvent(
      new CustomEvent("roc-recording-state", { detail: { anyRecording } }),
    );
  }, [anyRecording]);

  const settingsStream = settingsId != null ? streams.find((s) => s.id === settingsId) ?? null : null;
  const visibleStreams = sortByChannelId(streams.filter((s) => showEncodeCard(workflows, s.id)));

  return (
    <>
      <section className="io-section">
        <div className="io-section-head">
          <h2 className="io-section-title">Encode</h2>
        </div>

      {loading && visibleStreams.length === 0 ? (
        <div className="loading">
          <span>…</span>
        </div>
      ) : visibleStreams.length === 0 ? null : (
      <div className="cards-grid">
        {visibleStreams.map((s) => {
          const rec = recordings[s.id];
          const proxyRecOn = rec?.proxy?.status === "recording";
          const hqRecOn =
            rec?.hq?.status === "recording" ||
            (rec?.hq == null && rec?.proxy == null && rec?.status === "recording");
          const isRecording = proxyRecOn || hqRecOn;
          const srtOn = srtById[s.id]?.status === "streaming";
          const activePreset = presets.find((p) => p.id === s.encode_preset);
          const activeRecPreset = presets.find(
            (p) => p.id === (s.record_preset || s.encode_preset),
          );
          const cat = rec?.category || "_unsorted";
          const captureOn = isCaptureOn(s.status);
          const hasSignal = s.status === "running";
          const tslText = s.tsl_text?.trim();
          const proxyLabel = activePreset?.label || s.encode_preset || null;
          const recLabel =
            activeRecPreset?.label || s.record_preset || s.encode_preset || null;
          const signalLabel = shortSignalFormat(s.format);
          const actionError = cardError[s.id];
          return (
            <div key={s.id} className={`card-panel ${s.status}`}>
              <div className="card-stage">
                <AudioMeters channelId={s.id} bus="encode" silent={!hasSignal}>
                <div
                  className="card-thumb"
                  role="button"
                  tabIndex={captureOn ? 0 : -1}
                  onClick={() => captureOn && setPreview({ id: s.id, pair: 0 })}
                  onKeyDown={(e) => {
                    if (!captureOn) return;
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      setPreview({ id: s.id, pair: 0 });
                    }
                  }}
                  title={captureOn ? "Open preview" : undefined}
                >
                  <Thumbnail id={s.id} active={captureOn} lostSignal={!hasSignal && captureOn} />
                  {rec?.schedule && (
                    <div className="thumb-sched" title={`Scheduled ${formatSchedBadge(rec.schedule)}`}>
                      <div className={`sched-badge${rec.schedule.phase === "waiting" ? " waiting" : ""}`}>
                        {formatSchedBadge(rec.schedule)}
                      </div>
                    </div>
                  )}
                  {tslText && hasSignal && (
                    <div className="thumb-tsl-overlay">
                      <div className="tsl-badge" title={`TSL ${s.tsl_index ?? s.id}`}>
                        {tslText}
                      </div>
                    </div>
                  )}
                  {(isRecording || srtOn) && (
                    <div className="thumb-badges">
                      {proxyRecOn && (
                        <div className="rec-badge" title="Proxy recording">
                          REC PROXY · {formatElapsed(rec?.proxy?.elapsed_sec)}
                        </div>
                      )}
                      {hqRecOn && (
                        <div className="rec-badge" title="HQ recording">
                          REC HQ · {formatElapsed(rec?.hq?.elapsed_sec)}
                        </div>
                      )}
                      {srtOn && (
                        <div
                          className={`stream-badge${srtById[s.id]?.sending ? "" : " waiting"}`}
                          title={srtById[s.id]?.publish_url || "SRT"}
                        >
                          {srtById[s.id]?.sending
                            ? `SRT · ${formatBitrate(srtById[s.id]?.bitrate_kbps)}`
                            : "SRT …"}
                        </div>
                      )}
                    </div>
                  )}
                </div>
                </AudioMeters>
              </div>

              <div className="card-footer">
                <span
                  className={`card-channel-num ${s.status}`}
                  title={s.name || `Input ${s.id}`}
                >
                  {s.id}
                </span>
                <div className="card-footer-body">
                  <div className="card-top">
                    <div className="card-main">
                      <div className="card-identity-text">
                        <div className="card-name-row">
                          {editingNameId === s.id ? (
                            <input
                              ref={nameInputRef}
                              className="card-name-input"
                              value={nameDraft}
                              disabled={metaBusy[s.id]}
                              onChange={(e) => setNameDraft(e.target.value)}
                              onBlur={() => commitName(s.id)}
                              onKeyDown={(e) => {
                                if (e.key === "Enter") {
                                  e.preventDefault();
                                  (e.target as HTMLInputElement).blur();
                                } else if (e.key === "Escape") {
                                  e.preventDefault();
                                  setEditingNameId(null);
                                }
                              }}
                              aria-label="Channel name"
                            />
                          ) : (
                            <button
                              type="button"
                              className="card-name card-name-edit"
                              title="Click to rename"
                              disabled={metaBusy[s.id]}
                              onClick={() => beginEditName(s.id, rec?.name || `ch${s.id}`)}
                            >
                              {rec?.name || `ch${s.id}`}
                            </button>
                          )}
                          <select
                            className="card-category card-category-select"
                            value={cat}
                            disabled={metaBusy[s.id] || categories.length === 0}
                            title="Change folder"
                            aria-label="Category"
                            onChange={(e) => changeCategory(s.id, e.target.value)}
                          >
                            {!categories.some((c) => c.name === cat) && (
                              <option value={cat}>
                                {cat === "_unsorted" ? "Unsorted" : cat}
                              </option>
                            )}
                            {categories.map((c) => (
                              <option key={c.name} value={c.name}>
                                {c.name === "_unsorted" ? "Unsorted" : c.name}
                              </option>
                            ))}
                          </select>
                        </div>
                        <div className="card-meta-row">
                          <span
                            className="card-meta"
                            title={
                              s.status === "waiting"
                                ? signalLabel
                                  ? `No signal (last: ${signalLabel})`
                                  : "No signal"
                                : s.format || undefined
                            }
                          >
                            {s.status === "waiting" ? (
                              <span className="card-meta-item card-meta-waiting">No signal</span>
                            ) : signalLabel ? (
                              <span className="card-meta-item card-meta-format">{signalLabel}</span>
                            ) : (
                              <span className="card-meta-item">—</span>
                            )}
                          </span>
                        </div>
                        <div
                          className="card-presets"
                          title={`Proxy ${proxyLabel || "—"} · HQ ${recLabel || "—"}`}
                        >
                          <span className="card-preset-line">
                            <span className="card-preset-role">Proxy</span>
                            <span className="card-preset-value">{proxyLabel || "—"}</span>
                          </span>
                          <span className="card-preset-line">
                            <span className="card-preset-role">HQ</span>
                            <span className="card-preset-value">{recLabel || "—"}</span>
                          </span>
                        </div>
                      </div>
                    </div>
                    <div className="card-actions">
                      <div className="card-actions-primary">
                        <button
                          type="button"
                          className={`rec-btn ${proxyRecOn ? "recording" : "idle"}`}
                          onClick={() => toggleProxyRecording(s.id)}
                          disabled={proxyRecBusy[s.id] || (!hasSignal && !proxyRecOn)}
                          title={
                            proxyRecOn
                              ? "Stop proxy recording"
                              : !hasSignal
                                ? "No signal"
                                : `Start proxy recording (${proxyLabel || "proxy"})`
                          }
                        >
                          {proxyRecBusy[s.id] ? "…" : "REC PROXY"}
                        </button>
                        <button
                          type="button"
                          className={`rec-btn ${hqRecOn ? "recording" : "idle"}`}
                          onClick={() => toggleHqRecording(s.id)}
                          disabled={hqRecBusy[s.id] || (!hasSignal && !hqRecOn)}
                          title={
                            hqRecOn
                              ? "Stop HQ recording"
                              : !hasSignal
                                ? "No signal"
                                : `Start HQ recording (${recLabel || "HQ"})`
                          }
                        >
                          {hqRecBusy[s.id] ? "…" : "REC HQ"}
                        </button>
                        <button
                          type="button"
                          className={`stream-btn ${srtOn ? "streaming" : "idle"}`}
                          onClick={() => toggleSrt(s.id)}
                          disabled={srtBusy[s.id] || (!hasSignal && !srtOn)}
                          title={
                            srtOn
                              ? srtById[s.id]?.publish_url || "Stop SRT"
                              : !hasSignal
                                ? "No signal"
                                : "Start SRT"
                          }
                        >
                          {srtBusy[s.id] ? "…" : "SRT"}
                        </button>
                      </div>
                      <div className="card-actions-tools">
                        {captureOn && (
                          <ListenButton
                            pair={preview?.id === s.id ? preview.pair : null}
                            onChange={(p) => {
                              if (p == null) {
                                setPreview(null);
                                return;
                              }
                              setPreview({ id: s.id, pair: p });
                            }}
                          />
                        )}
                        <button
                          type="button"
                          className="badge settings-btn"
                          onClick={() => setSettingsId(s.id)}
                          title="Channel settings"
                          aria-label="Settings"
                        >
                          ⚙
                        </button>
                      </div>
                    </div>
                  </div>
                  {actionError && (
                    <div className="card-error" title={actionError}>
                      <button
                        type="button"
                        className="card-error-dismiss"
                        onClick={() => clearCardError(s.id)}
                        aria-label="Dismiss"
                      >
                        ×
                      </button>
                      {actionError}
                    </div>
                  )}
                </div>
              </div>
            </div>
          );
        })}
      </div>
      )}
      </section>

      <PreviewModal
        open={preview != null}
        channelId={preview?.id ?? 0}
        channelName={
          preview != null
            ? recordings[preview.id]?.name || streams.find((x) => x.id === preview.id)?.name || `ch${preview.id}`
            : ""
        }
        initialPair={preview?.pair ?? 0}
        onClose={() => setPreview(null)}
      />

      <ChannelSettingsModal
        open={settingsId != null}
        stream={settingsStream}
        recording={settingsId != null ? recordings[settingsId] ?? null : null}
        presets={presets}
        categories={categories}
        onClose={() => setSettingsId(null)}
        onSaved={() => {}}
      />
    </>
  );
}
