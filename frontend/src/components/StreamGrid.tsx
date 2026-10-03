"use client";

import { useEffect, useState } from "react";
import {
  fetchEncodePresets,
  fetchLibraryCategories,
  startRecording,
  stopRecording,
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
import HlsPreview from "@/components/HlsPreview";
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

function formatSchedBadge(sch: RecordingSchedule): string {
  const hm = (iso: string) => {
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return "--:--";
    return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
  };
  const range = `${hm(sch.start_at)}–${hm(sch.stop_at)}`;
  if (sch.phase === "waiting") return `SCHED wait · ${range}`;
  return `SCHED ${range}`;
}

export default function StreamGrid() {
  const { loading, streams, recordings, srtById } = useDashboard();
  const [presets, setPresets] = useState<EncodePreset[]>([]);
  const [categories, setCategories] = useState<LibraryCategory[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [recBusy, setRecBusy] = useState<Record<number, boolean>>({});
  const [srtBusy, setSrtBusy] = useState<Record<number, boolean>>({});
  const [listenPair, setListenPair] = useState<Record<number, number | null>>({});
  const [settingsId, setSettingsId] = useState<number | null>(null);
  const { workflows } = useWorkflows();

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
      .catch((e) => setError(String(e)));
    refreshCategories();
    window.addEventListener("roc-presets-changed", refreshPresets);
    window.addEventListener("roc-library-changed", refreshCategories);
    return () => {
      window.removeEventListener("roc-presets-changed", refreshPresets);
      window.removeEventListener("roc-library-changed", refreshCategories);
    };
  }, []);

  const toggleRecording = async (id: number) => {
    setRecBusy((b) => ({ ...b, [id]: true }));
    try {
      if (recordings[id]?.status === "recording") await stopRecording(id);
      else await startRecording(id);
    } catch (e) {
      setError(String(e));
    } finally {
      setRecBusy((b) => ({ ...b, [id]: false }));
    }
  };

  const toggleSrt = async (id: number) => {
    setSrtBusy((b) => ({ ...b, [id]: true }));
    try {
      if (srtById[id]?.status === "streaming") await stopSrt(id);
      else await startSrt(id);
    } catch (e) {
      setError(String(e));
    } finally {
      setSrtBusy((b) => ({ ...b, [id]: false }));
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
      {error && (
        <div className="error-message">
          {error}
          <button type="button" className="error-dismiss" onClick={() => setError(null)} aria-label="Dismiss">
            ×
          </button>
        </div>
      )}

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
          const isRecording = rec?.status === "recording";
          const isEncoding = isRecording && !!rec?.encoding;
          const isListeningPair = listenPair[s.id] ?? null;
          const srtOn = srtById[s.id]?.status === "streaming";
          const activePreset = presets.find((p) => p.id === s.encode_preset);
          const cat = rec?.category || "_unsorted";
          const captureOn = isCaptureOn(s.status);
          const hasSignal = s.status === "running";
          const tslText = s.tsl_text?.trim();
          return (
            <div key={s.id} className={`card-panel ${s.status}`}>
              <div className="card-stage">
                <AudioMeters channelId={s.id} bus="encode">
                <div className="card-thumb">
                  <HlsPreview
                    active={captureOn}
                    listenPair={isListeningPair}
                    playlistPath={`/hls/${s.id}/preview.m3u8`}
                  />
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
                      {isEncoding && (
                        <div className="rec-badge">
                          REC · {formatElapsed(rec?.elapsed_sec)}
                        </div>
                      )}
                      {isRecording && !isEncoding && (
                        <div className="rec-badge starting">REC …</div>
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
                <div className="card-top">
                  <div className="card-identity">
                    <span
                      className={`card-channel-num ${s.status}`}
                      title={s.name || `Input ${s.id}`}
                    >
                      {s.id}
                    </span>
                    <div className="card-identity-text">
                      <span className="card-name" title={rec?.name || `ch${s.id}`}>
                        {rec?.name || `ch${s.id}`}
                      </span>
                      <div
                        className="card-meta"
                        title={[
                          s.format || (s.status === "waiting" ? "No signal" : null),
                          cat === "_unsorted" ? "Unsorted" : cat,
                          activePreset?.label || s.encode_preset || null,
                        ]
                          .filter(Boolean)
                          .join(" · ")}
                      >
                        {s.format ? (
                          <>
                            <span className="card-meta-item card-meta-format">{s.format}</span>
                            <span className="card-meta-sep">·</span>
                          </>
                        ) : s.status === "waiting" ? (
                          <>
                            <span className="card-meta-item card-meta-waiting">No signal</span>
                            <span className="card-meta-sep">·</span>
                          </>
                        ) : null}
                        <span className="card-meta-item">
                          {cat === "_unsorted" ? "Unsorted" : cat}
                        </span>
                        <span className="card-meta-sep">·</span>
                        <span className="card-meta-item">
                          {activePreset?.label || s.encode_preset || "—"}
                        </span>
                      </div>
                    </div>
                  </div>
                  <div className="card-actions">
                    <>
                      <button
                          type="button"
                          className={`rec-btn ${isRecording ? "recording" : "idle"}`}
                          onClick={() => toggleRecording(s.id)}
                          disabled={recBusy[s.id] || (!hasSignal && !isRecording)}
                          title={
                            isRecording
                              ? "Stop recording"
                              : !hasSignal
                                ? "No signal"
                                : "Start recording"
                          }
                        >
                          {recBusy[s.id] ? "…" : "REC"}
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
                          {srtBusy[s.id] ? "…" : "STREAM"}
                        </button>
                    </>
                    {captureOn && (
                      <ListenButton
                        pair={isListeningPair}
                        onChange={(p) => setListenPair((prev) => ({ ...prev, [s.id]: p }))}
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
            </div>
          );
        })}
      </div>
      )}
      </section>

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
