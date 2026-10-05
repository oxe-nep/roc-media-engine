"use client";

import { useEffect, useRef, useState } from "react";
import {
  isPlayoutOn,
  isPlayoutPaused,
  pausePlayout,
  resumePlayout,
  seekPlayout,
  startPlayout,
  stopPlayout,
  updatePlayoutClient,
  type PlayoutClient,
} from "@/lib/api";
import { showDecodeCard } from "@/lib/workflow";
import { sortByChannelId } from "@/lib/sortChannels";
import { useWorkflows } from "@/hooks/useWorkflows";
import { useDashboard } from "@/hooks/useDashboard";
import AudioMeters from "@/components/AudioMeters";
import DecodeSettingsModal from "@/components/DecodeSettingsModal";

function formatClock(sec?: number): string {
  if (sec == null || !Number.isFinite(sec) || sec < 0) return "--:--";
  const s = Math.floor(sec);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const r = s % 60;
  if (h > 0) {
    return `${h}:${String(m).padStart(2, "0")}:${String(r).padStart(2, "0")}`;
  }
  return `${m}:${String(r).padStart(2, "0")}`;
}

function formatDisplay(code?: string): string {
  if (!code) return "—";
  if (code === "auto") return "Auto";
  const map: Record<string, string> = {
    Hi50: "1080i50",
    Hp50: "1080p50",
    Hi25: "1080i25",
    Hp25: "1080p25",
    "Hp59.94": "1080p59.94",
    Hp5994: "1080p59.94",
    "Hi59.94": "1080i59.94",
    Hi5994: "1080i59.94",
    Hp60: "1080p60",
    Hp30: "1080p30",
    Hp24: "1080p24",
  };
  return map[code] || code;
}

function basename(path: string): string {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
}

function cardTitle(c: PlayoutClient): string {
  if (c.source === "file") {
    if (c.file_name) return basename(c.file_name);
    return c.name?.trim() || `Decode ${c.id}`;
  }
  if (c.mode === "caller" && c.target?.trim()) return c.target.trim();
  if (c.mode === "listener") return `SRT :${c.port}`;
  return c.name?.trim() || `Decode ${c.id}`;
}

function cardMeta(c: PlayoutClient): string {
  const bits = [formatDisplay(c.format_code), (c.source || "srt").toUpperCase()];
  if (c.source === "file" && c.loop) bits.push("LOOP");
  return bits.join(" · ");
}

function statusLabel(c: PlayoutClient, on: boolean, paused: boolean): string {
  if (paused) return "Paused";
  if (c.status === "waiting") return "Waiting";
  if (c.status === "error") return "Error";
  if (on) return "Playing";
  return "Idle";
}

function stageLines(c: PlayoutClient): string[] {
  const lines: string[] = [];
  if (c.source === "file") {
    const name = c.file_name ? basename(c.file_name) : c.file_id || "No file";
    lines.push(name);
    if (c.loop) lines.push("Loop on");
  } else if (c.mode === "listener") {
    lines.push(`SRT listener :${c.port || "—"}`);
  } else {
    lines.push(c.target?.trim() || "SRT caller (no target)");
  }
  lines.push(formatDisplay(c.format_code));
  if (c.device_label || c.device) {
    lines.push(c.device_label || c.device);
  }
  return lines;
}

function FileTimeline({
  client: c,
  disabled,
  onError,
}: {
  client: PlayoutClient;
  disabled?: boolean;
  onError: (msg: string) => void;
}) {
  const duration = Math.max(0, c.duration_sec ?? 0);
  const livePos = Math.max(0, c.elapsed_sec ?? 0);
  const markIn = Math.max(0, c.mark_in_sec ?? 0);
  const markOut =
    c.mark_out_sec != null && Number.isFinite(c.mark_out_sec) ? Math.max(0, c.mark_out_sec) : null;
  const [scrub, setScrub] = useState<number | null>(null);
  const scrubbing = scrub != null;
  const displayPos = scrubbing ? scrub : livePos;
  const seekTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const on = isPlayoutOn(c.status);

  useEffect(() => {
    if (!scrubbing) return;
    // Drop local scrub once dashboard catches up (±0.35s).
    if (Math.abs(livePos - scrub) < 0.35) setScrub(null);
  }, [livePos, scrub, scrubbing]);

  useEffect(() => {
    return () => {
      if (seekTimer.current) clearTimeout(seekTimer.current);
    };
  }, []);

  if (duration <= 0 && !c.file_id) return null;

  const pct = duration > 0 ? Math.min(100, (displayPos / duration) * 100) : 0;
  const inPct = duration > 0 ? Math.min(100, (markIn / duration) * 100) : 0;
  const outPct = markOut != null && duration > 0 ? Math.min(100, (markOut / duration) * 100) : 100;

  const queueSeek = (sec: number) => {
    setScrub(sec);
    if (!on) return;
    if (seekTimer.current) clearTimeout(seekTimer.current);
    seekTimer.current = setTimeout(() => {
      seekPlayout(c.id, sec).catch((e) => onError(String(e)));
    }, 90);
  };

  const setMarkIn = async () => {
    try {
      await updatePlayoutClient(c.id, { mark_in_sec: displayPos });
    } catch (e) {
      onError(String(e));
    }
  };

  const setMarkOut = async () => {
    try {
      await updatePlayoutClient(c.id, { mark_out_sec: displayPos });
    } catch (e) {
      onError(String(e));
    }
  };

  const clearMarks = async () => {
    try {
      await updatePlayoutClient(c.id, { mark_in_sec: 0, mark_out_sec: null });
    } catch (e) {
      onError(String(e));
    }
  };

  return (
    <div className="file-timeline">
      <div className="file-timeline-track">
        <div className="file-timeline-range" style={{ left: `${inPct}%`, width: `${Math.max(0, outPct - inPct)}%` }} />
        <input
          type="range"
          className="file-timeline-scrub"
          min={0}
          max={duration || 1}
          step={0.04}
          value={Math.min(displayPos, duration || 1)}
          disabled={disabled || duration <= 0}
          aria-label="Scrub timeline"
          onChange={(e) => queueSeek(Number(e.target.value))}
        />
        <div className="file-timeline-playhead" style={{ left: `${pct}%` }} />
      </div>
      <div className="file-timeline-row">
        <span className="file-timeline-clock">
          {formatClock(displayPos)}
          <span className="file-timeline-clock-sep">/</span>
          {formatClock(duration)}
        </span>
        <div className="file-timeline-marks">
          <button type="button" className="ctrl-btn tiny" disabled={disabled || duration <= 0} onClick={() => void setMarkIn()} title="Mark in at playhead">
            IN
          </button>
          <button type="button" className="ctrl-btn tiny" disabled={disabled || duration <= 0} onClick={() => void setMarkOut()} title="Mark out at playhead">
            OUT
          </button>
          <button
            type="button"
            className="ctrl-btn tiny"
            disabled={disabled || (markIn <= 0 && markOut == null)}
            onClick={() => void clearMarks()}
            title="Clear in/out"
          >
            CLR
          </button>
        </div>
      </div>
      {(markIn > 0 || markOut != null) && (
        <div className="file-timeline-mark-meta">
          In {formatClock(markIn)}
          {markOut != null ? ` · Out ${formatClock(markOut)}` : ""}
        </div>
      )}
    </div>
  );
}

export default function DecodeGrid() {
  const { loading, playout: clients } = useDashboard();
  const [cardError, setCardError] = useState<Record<number, string>>({});
  const [busy, setBusy] = useState<Record<number, boolean>>({});
  const [settingsId, setSettingsId] = useState<number | null>(null);
  const { workflows } = useWorkflows();

  const withBusy = async (id: number, fn: () => Promise<unknown>) => {
    setBusy((b) => ({ ...b, [id]: true }));
    setCardError((e) => {
      const next = { ...e };
      delete next[id];
      return next;
    });
    try {
      await fn();
    } catch (e) {
      setCardError((prev) => ({ ...prev, [id]: String(e) }));
    } finally {
      setBusy((b) => ({ ...b, [id]: false }));
    }
  };

  const toggleLoop = async (c: PlayoutClient) => {
    const next = !c.loop;
    try {
      await updatePlayoutClient(c.id, { loop: next });
    } catch (e) {
      setCardError((prev) => ({ ...prev, [c.id]: String(e) }));
    }
  };

  const settingsClient = settingsId != null ? clients.find((c) => c.id === settingsId) ?? null : null;
  const visibleClients = sortByChannelId(clients.filter((c) => showDecodeCard(workflows, c.id)));

  if (!loading && visibleClients.length === 0) {
    return null;
  }

  return (
    <section className="io-section">
      <div className="io-section-head">
        <h2 className="io-section-title">Decode</h2>
      </div>

      {loading && clients.length === 0 ? (
        <div className="loading">
          <span>…</span>
        </div>
      ) : (
        <div className="cards-grid">
          {visibleClients.map((c) => {
            const on = isPlayoutOn(c.status);
            const paused = isPlayoutPaused(c.status);
            const isFile = c.source === "file";
            const title = cardTitle(c);
            const actionError = cardError[c.id];
            const engineError = c.error?.trim() || "";
            const showError = actionError || engineError;
            return (
              <div key={c.id} className={`card-panel ${c.status}`}>
                <div className="card-stage">
                  <AudioMeters channelId={c.id} bus="playout" channels={2} silent={!on}>
                    <div className={`card-thumb card-thumb-meta${on ? " on" : ""}`}>
                      <div className="decode-meta">
                        <span className={`decode-meta-status ${c.status}`}>
                          {statusLabel(c, on, paused)}
                        </span>
                        {stageLines(c).map((line, i) => (
                          <span key={`${i}-${line}`} className="decode-meta-line" title={line}>
                            {line}
                          </span>
                        ))}
                      </div>
                    </div>
                  </AudioMeters>
                </div>

                <div className="card-footer">
                  <span className={`card-channel-num ${c.status}`} title={`Output ${c.id}`}>
                    {c.id}
                  </span>
                  <div className="card-footer-body">
                    <div className="card-top">
                      <div className="card-main">
                        <div className="card-identity-text">
                          <span className="card-name" title={title}>
                            {title}
                          </span>
                          <div className="card-meta" title={cardMeta(c)}>
                            <span className="card-meta-item">{formatDisplay(c.format_code)}</span>
                            <span className="card-meta-sep">·</span>
                            <span className="card-meta-item">{(c.source || "srt").toUpperCase()}</span>
                          </div>
                        </div>
                      </div>
                      <div className="card-actions">
                        {isFile ? (
                          <div className="transport">
                            {!on || paused ? (
                              <button
                                type="button"
                                className="ctrl-btn"
                                disabled={busy[c.id] || (!c.file_id && !paused)}
                                onClick={() =>
                                  withBusy(c.id, async () => {
                                    if (paused) await resumePlayout(c.id);
                                    else await startPlayout(c.id);
                                  })
                                }
                                title={paused ? "Resume" : "Play"}
                              >
                                {busy[c.id] ? "…" : "PLAY"}
                              </button>
                            ) : (
                              <button
                                type="button"
                                className="ctrl-btn primary"
                                disabled={busy[c.id]}
                                onClick={() => withBusy(c.id, async () => pausePlayout(c.id))}
                                title="Pause"
                              >
                                {busy[c.id] ? "…" : "PAUSE"}
                              </button>
                            )}
                            <button
                              type="button"
                              className="ctrl-btn"
                              disabled={busy[c.id] || !on}
                              onClick={() => withBusy(c.id, async () => stopPlayout(c.id))}
                              title="Stop"
                            >
                              STOP
                            </button>
                            <button
                              type="button"
                              className={`ctrl-btn${c.loop ? " on" : ""}`}
                              disabled={busy[c.id]}
                              onClick={() => toggleLoop(c)}
                              title={c.loop ? "Loop on" : "Loop off"}
                            >
                              LOOP
                            </button>
                          </div>
                        ) : (
                          <button
                            type="button"
                            className={`stream-btn ${on ? "streaming" : "idle"}`}
                            onClick={() =>
                              withBusy(c.id, async () => {
                                if (on) await stopPlayout(c.id);
                                else await startPlayout(c.id);
                              })
                            }
                            disabled={busy[c.id]}
                            title={on ? "Stop" : "Start"}
                          >
                            {busy[c.id] ? "…" : on ? "STOP" : "START"}
                          </button>
                        )}
                        <button
                          type="button"
                          className="badge settings-btn"
                          onClick={() => setSettingsId(c.id)}
                          aria-label="Settings"
                        >
                          ⚙
                        </button>
                      </div>
                    </div>
                    {isFile && (
                      <FileTimeline
                        client={c}
                        disabled={!!busy[c.id]}
                        onError={(msg) => setCardError((prev) => ({ ...prev, [c.id]: msg }))}
                      />
                    )}
                    {showError && (
                      <div className="card-error" title={showError}>
                        <button
                          type="button"
                          className="card-error-dismiss"
                          onClick={() =>
                            setCardError((prev) => {
                              const next = { ...prev };
                              delete next[c.id];
                              return next;
                            })
                          }
                          aria-label="Dismiss"
                        >
                          ×
                        </button>
                        {actionError || engineError}
                      </div>
                    )}
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      )}

      <DecodeSettingsModal
        open={settingsId != null}
        client={settingsClient}
        onClose={() => setSettingsId(null)}
        onSaved={() => {}}
      />
    </section>
  );
}
