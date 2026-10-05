"use client";

import { useEffect, useRef, useState } from "react";
import {
  fetchEncodePresets,
  setRecordingName,
  startSrt,
  stopSrt,
  updateTcLoop,
  type EncodePreset,
} from "@/lib/api";
import { isProxyPreset } from "@/lib/presetRoles";
import { tcCardStatusMeta, tcIsActive, tcPreviewHasSignal, tcSourceLabel } from "@/lib/tcUi";
import { showTcCard } from "@/lib/workflow";
import { sortByChannelId } from "@/lib/sortChannels";
import { useWorkflows } from "@/hooks/useWorkflows";
import { useDashboard } from "@/hooks/useDashboard";
import Thumbnail from "@/components/Thumbnail";
import AudioMeters from "@/components/AudioMeters";
import ListenButton from "@/components/ListenButton";
import PreviewModal from "@/components/PreviewModal";
import TcSettingsModal from "@/components/TcSettingsModal";

function formatBitrate(kbps?: number): string {
  if (kbps == null || !Number.isFinite(kbps) || kbps <= 0) return "…";
  if (kbps >= 1000) return `${(kbps / 1000).toFixed(1)} Mb/s`;
  return `${Math.round(kbps)} kb/s`;
}

export default function TcGrid() {
  const { loading, streams, tcById, srtById, recordings } = useDashboard();
  const [cardError, setCardError] = useState<Record<number, string>>({});
  const [busy, setBusy] = useState<Record<number, boolean>>({});
  const [srtBusy, setSrtBusy] = useState<Record<number, boolean>>({});
  const [metaBusy, setMetaBusy] = useState<Record<number, boolean>>({});
  const [listenPair, setListenPair] = useState<Record<number, number | null>>({});
  const [preview, setPreview] = useState<{ id: number; pair: number } | null>(null);
  const [settingsId, setSettingsId] = useState<number | null>(null);
  const [presets, setPresets] = useState<EncodePreset[]>([]);
  const [editingNameId, setEditingNameId] = useState<number | null>(null);
  const [nameDraft, setNameDraft] = useState("");
  const nameInputRef = useRef<HTMLInputElement>(null);
  const { workflows } = useWorkflows();

  const tcStreams = sortByChannelId(streams.filter((s) => showTcCard(workflows, s.id)));
  const channelIds = tcStreams.map((s) => s.id);

  useEffect(() => {
    fetchEncodePresets()
      .then(setPresets)
      .catch(() => {});
    const refresh = () => {
      fetchEncodePresets()
        .then(setPresets)
        .catch(() => {});
    };
    window.addEventListener("roc-presets-changed", refresh);
    return () => window.removeEventListener("roc-presets-changed", refresh);
  }, []);

  useEffect(() => {
    if (editingNameId == null) return;
    nameInputRef.current?.focus();
    nameInputRef.current?.select();
  }, [editingNameId]);

  const clearCardError = (id: number) => {
    setCardError((prev) => {
      if (!prev[id]) return prev;
      const next = { ...prev };
      delete next[id];
      return next;
    });
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
      setCardError((prevErr) => ({ ...prevErr, [id]: String(e) }));
    } finally {
      setMetaBusy((b) => ({ ...b, [id]: false }));
    }
  };

  const stopTc = async (id: number) => {
    setBusy((b) => ({ ...b, [id]: true }));
    clearCardError(id);
    try {
      await updateTcLoop(id, { enabled: false });
    } catch (e) {
      setCardError((prev) => ({ ...prev, [id]: String(e) }));
    } finally {
      setBusy((b) => ({ ...b, [id]: false }));
    }
  };

  const startTc = async (id: number) => {
    setBusy((b) => ({ ...b, [id]: true }));
    clearCardError(id);
    try {
      await updateTcLoop(id, { enabled: true });
    } catch (e) {
      setCardError((prev) => ({ ...prev, [id]: String(e) }));
    } finally {
      setBusy((b) => ({ ...b, [id]: false }));
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

  if (!loading && channelIds.length === 0) {
    return null;
  }

  const settingsStream = settingsId != null ? streams.find((s) => s.id === settingsId) : null;

  return (
    <section className="io-section io-section-tc">
      <div className="io-section-head">
        <h2 className="io-section-title">TC</h2>
      </div>

      {loading && channelIds.length === 0 ? (
        <div className="loading">
          <span>…</span>
        </div>
      ) : (
        <div className="cards-grid">
          {tcStreams.map((s) => {
              const tc = tcById[s.id];
              const rec = recordings[s.id];
              const displayName = rec?.name || `ch${s.id}`;
              const tcOn = tcIsActive(tc);
              const tcLive = tcPreviewHasSignal(tc);
              const srtOn = srtById[s.id]?.status === "streaming";
              const listenAt = listenPair[s.id] ?? null;
              const tslText = s.tsl_text?.trim();
              const actionError = cardError[s.id];
              const engineError = tc?.error?.trim() || "";
              const showError = actionError || engineError;
              const activePreset = presets.find((p) => p.id === s.encode_preset);
              const proxyLabel =
                (activePreset && isProxyPreset(activePreset) ? activePreset.label : null) ||
                s.encode_preset ||
                null;
              const numClass = tcLive
                ? "running"
                : tc?.status === "error"
                  ? "error"
                  : tcOn
                    ? "waiting"
                    : "stopped";
              return (
                <div
                  key={s.id}
                  className={`card-panel ${s.status}${tcOn ? " tc-active" : ""}${tcLive ? " tc-live" : ""}`}
                >
                  <div className="card-stage">
                    <AudioMeters channelId={s.id} bus="playout" silent={!tcLive}>
                    <div
                      className="card-thumb"
                      role="button"
                      tabIndex={tcOn ? 0 : -1}
                      onClick={() => tcOn && setPreview({ id: s.id, pair: listenAt ?? 0 })}
                      onKeyDown={(e) => {
                        if (!tcOn) return;
                        if (e.key === "Enter" || e.key === " ") {
                          e.preventDefault();
                          setPreview({ id: s.id, pair: listenAt ?? 0 });
                        }
                      }}
                      title={tcOn ? "Open preview" : undefined}
                    >
                      <Thumbnail
                        id={s.id}
                        active={tcOn}
                        lostSignal={!tcLive && tcOn && !showError}
                        path={`/thumb/playout/${s.id}`}
                      />
                      {showError && (
                        <div className="thumb-error-overlay" title={showError} role="alert">
                          <button
                            type="button"
                            className="thumb-error-dismiss"
                            onClick={(e) => {
                              e.stopPropagation();
                              clearCardError(s.id);
                            }}
                            aria-label="Dismiss"
                          >
                            ×
                          </button>
                          <p className="thumb-error-msg">{actionError || engineError}</p>
                        </div>
                      )}
                      {tslText && tcOn && !showError && (
                        <div className="thumb-tsl-overlay">
                          <div className="tsl-badge" title={`TSL ${s.tsl_index ?? s.id}`}>
                            {tslText}
                          </div>
                        </div>
                      )}
                      {srtOn && !showError && (
                        <div className="thumb-badges">
                          <div
                            className={`stream-badge${srtById[s.id]?.sending ? "" : " waiting"}`}
                            title={srtById[s.id]?.publish_url || "SRT"}
                          >
                            {srtById[s.id]?.sending
                              ? `SRT · ${formatBitrate(srtById[s.id]?.bitrate_kbps)}`
                              : "SRT …"}
                          </div>
                        </div>
                      )}
                    </div>
                    </AudioMeters>
                  </div>

                  <div className="card-footer">
                    <span className={`card-channel-num ${numClass}`}>{s.id}</span>
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
                                onClick={() => beginEditName(s.id, displayName)}
                              >
                                {displayName}
                              </button>
                            )}
                          </div>
                          <div
                            className="card-meta"
                            title={
                              tcOn
                                ? [
                                    tcSourceLabel(tc?.source, tc?.udp_port, s.id),
                                    tc?.format || tc?.mode,
                                  ]
                                    .filter(Boolean)
                                    .join(" · ")
                                : undefined
                            }
                          >
                            <span className="card-meta-item card-meta-tc">{tcCardStatusMeta(tc, tcLive)}</span>
                          </div>
                          {proxyLabel && (
                            <div className="card-presets" title={`Proxy ${proxyLabel}`}>
                              <span className="card-preset-line">
                                <span className="card-preset-role">Proxy</span>
                                <span className="card-preset-value">{proxyLabel}</span>
                              </span>
                            </div>
                          )}
                        </div>
                      </div>
                      <div className="card-actions">
                        <div className="card-actions-primary">
                          {tcOn ? (
                            <button
                              type="button"
                              className="tc-stop-btn"
                              disabled={busy[s.id]}
                              onClick={() => stopTc(s.id)}
                            >
                              {busy[s.id] ? "…" : "STOP"}
                            </button>
                          ) : (
                            <button
                              type="button"
                              className="tc-start-btn"
                              disabled={busy[s.id]}
                              onClick={() => startTc(s.id)}
                            >
                              {busy[s.id] ? "…" : "START"}
                            </button>
                          )}
                          <button
                            type="button"
                            className={`stream-btn ${srtOn ? "streaming" : "idle"}`}
                            onClick={() => toggleSrt(s.id)}
                            disabled={srtBusy[s.id] || (!tcLive && !srtOn)}
                            title={
                              srtOn
                                ? srtById[s.id]?.publish_url || "Stop SRT"
                                : "Start SRT (burned-in proxy)"
                            }
                          >
                            {srtBusy[s.id] ? "…" : "SRT"}
                          </button>
                        </div>
                        <div className="card-actions-tools">
                          {tcLive && (
                            <ListenButton
                              pair={listenAt}
                              onChange={(p) => {
                                setListenPair((prev) => ({ ...prev, [s.id]: p }));
                                if (p != null) setPreview({ id: s.id, pair: p });
                              }}
                            />
                          )}
                          <button
                            type="button"
                            className="badge settings-btn"
                            onClick={() => setSettingsId(s.id)}
                            title="TC settings"
                            aria-label="Settings"
                          >
                            ⚙
                          </button>
                        </div>
                      </div>
                    </div>
                    </div>
                  </div>
                </div>
              );
            })}
        </div>
      )}

      <TcSettingsModal
        open={settingsId != null}
        channelId={settingsId}
        channelName={
          settingsId != null
            ? recordings[settingsId]?.name || `ch${settingsId}`
            : undefined
        }
        encodePreset={settingsStream?.encode_preset}
        onClose={() => setSettingsId(null)}
        onSaved={() => {}}
      />

      <PreviewModal
        open={preview != null}
        channelId={preview?.id ?? 0}
        channelName={
          preview
            ? recordings[preview.id]?.name ||
              streams.find((x) => x.id === preview.id)?.name ||
              `TC ${preview.id}`
            : ""
        }
        initialPair={preview?.pair ?? 0}
        onClose={() => setPreview(null)}
      />
    </section>
  );
}
