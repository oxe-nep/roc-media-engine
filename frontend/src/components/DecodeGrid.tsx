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

function shortSrt(url: string): string {
  const raw = url.trim();
  if (!raw) return "No target";
  return raw.replace(/^srt:\/\//i, "").split("?")[0] || raw;
}

function shortOut(device?: string, id?: number): string {
  const m = device?.match(/\((\d+)\)\s*$/);
  if (m?.[1]) return `OUT ${m[1]}`;
  if (id != null) return `OUT ${id}`;
  return "OUT";
}

function cardTitle(c: PlayoutClient): string {
  if (c.source === "file") {
    if (c.file_name) return basename(c.file_name);
    if (c.file_id) return basename(c.file_id);
    return "No file selected";
  }
  if (c.mode === "listener") return `Listen :${c.port || "—"}`;
  if (c.target?.trim()) return shortSrt(c.target);
  return "No SRT target";
}

function cardMetaBits(c: PlayoutClient): string[] {
  const bits: string[] = [];
  if (c.source === "file") {
    bits.push("File");
  } else if (c.mode === "listener") {
    bits.push("Listener");
  } else {
    bits.push("Caller");
  }
  bits.push(formatDisplay(c.format_code));
  bits.push(shortOut(c.device_label || c.device, c.id));
  if (c.source === "file" && c.loop) bits.push("Loop");
  if (c.source === "file" && ((c.mark_in_sec ?? 0) > 0 || c.mark_out_sec != null)) {
    bits.push(
      `In ${formatClock(c.mark_in_sec)}${
        c.mark_out_sec != null ? `–${formatClock(c.mark_out_sec)}` : ""
      }`,
    );
  }
  if (c.source !== "file" && (c.latency_ms ?? 0) > 0) {
    bits.push(`${c.latency_ms} ms`);
  }
  return bits;
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
  const on = isPlayoutOn(c.status);
  const paused = isPlayoutPaused(c.status);
  const [dragging, setDragging] = useState(false);
  const [scrub, setScrub] = useState<number | null>(null);
  const [localPos, setLocalPos] = useState(livePos);
  const anchorRef = useRef<{ t: number; pos: number } | null>(null);
  const seekTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (dragging) return;
    setLocalPos(livePos);
    if (on && !paused) {
      anchorRef.current = { t: performance.now(), pos: livePos };
    } else {
      anchorRef.current = null;
    }
  }, [livePos, dragging, on, paused]);

  useEffect(() => {
    if (dragging || !on || paused) return;
    let raf = 0;
    const tick = () => {
      const a = anchorRef.current;
      if (a) {
        const pred = a.pos + (performance.now() - a.t) / 1000;
        setLocalPos(duration > 0 ? Math.min(pred, duration) : pred);
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [dragging, on, paused, duration, c.id]);

  useEffect(() => {
    return () => {
      if (seekTimer.current) clearTimeout(seekTimer.current);
    };
  }, []);

  if (duration <= 0 && !c.file_id) return null;

  const displayPos = dragging && scrub != null ? scrub : localPos;
  const inPct = duration > 0 ? Math.min(100, (markIn / duration) * 100) : 0;
  const outPct = markOut != null && duration > 0 ? Math.min(100, (markOut / duration) * 100) : 100;

  const queueSeek = (sec: number) => {
    setScrub(sec);
    setLocalPos(sec);
    if (!on) return;
    if (seekTimer.current) clearTimeout(seekTimer.current);
    seekTimer.current = setTimeout(() => {
      seekPlayout(c.id, sec).catch((e) => onError(String(e)));
    }, 90);
  };

  return (
    <div className="file-timeline">
      <div className="file-timeline-track">
        <div
          className="file-timeline-range"
          style={{ left: `${inPct}%`, width: `${Math.max(0, outPct - inPct)}%` }}
        />
        <input
          type="range"
          className="file-timeline-scrub"
          min={0}
          max={duration || 1}
          step={0.04}
          value={Math.min(displayPos, duration || 1)}
          disabled={disabled || duration <= 0}
          aria-label="Scrub timeline"
          onPointerDown={() => setDragging(true)}
          onPointerUp={() => {
            setDragging(false);
            setScrub(null);
          }}
          onPointerCancel={() => {
            setDragging(false);
            setScrub(null);
          }}
          onChange={(e) => queueSeek(Number(e.target.value))}
        />
      </div>
      <div className="file-timeline-row">
        <span className="file-timeline-clock">
          {formatClock(displayPos)}
          <span className="file-timeline-clock-sep">/</span>
          {formatClock(duration)}
        </span>
        <div className="file-timeline-marks">
          <button
            type="button"
            className="ctrl-btn tiny"
            disabled={disabled || duration <= 0}
            onClick={() =>
              void updatePlayoutClient(c.id, { mark_in_sec: displayPos }).catch((e) =>
                onError(String(e)),
              )
            }
            title="Mark in"
          >
            IN
          </button>
          <button
            type="button"
            className="ctrl-btn tiny"
            disabled={disabled || duration <= 0}
            onClick={() =>
              void updatePlayoutClient(c.id, { mark_out_sec: displayPos }).catch((e) =>
                onError(String(e)),
              )
            }
            title="Mark out"
          >
            OUT
          </button>
          <button
            type="button"
            className="ctrl-btn tiny"
            disabled={disabled || (markIn <= 0 && markOut == null)}
            onClick={() =>
              void updatePlayoutClient(c.id, { mark_in_sec: 0, mark_out_sec: null }).catch((e) =>
                onError(String(e)),
              )
            }
            title="Clear in/out"
          >
            CLR
          </button>
        </div>
      </div>
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
    try {
      await updatePlayoutClient(c.id, { loop: !c.loop });
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
        <div className="cards-grid decode-grid">
          {visibleClients.map((c) => {
            const on = isPlayoutOn(c.status);
            const paused = isPlayoutPaused(c.status);
            const isFile = c.source === "file";
            const title = cardTitle(c);
            const meta = cardMetaBits(c);
            const actionError = cardError[c.id];
            const engineError = c.error?.trim() || "";
            const showError = actionError || engineError;
            return (
              <div key={c.id} className={`card-panel decode-card ${c.status}`}>
                <div className="decode-row">
                  <span className={`card-channel-num ${c.status}`} title={`Output ${c.id}`}>
                    {c.id}
                  </span>
                  <div className="decode-main">
                    <AudioMeters channelId={c.id} bus="playout" channels={2} silent={!on}>
                      <div className="decode-body">
                        <div className="card-top">
                          <div className="card-main">
                            <div className="card-identity-text">
                              <span className="card-name" title={title}>
                                {title}
                              </span>
                              <div className="card-meta" title={meta.join(" · ")}>
                                {meta.map((bit, i) => (
                                  <span key={`${bit}-${i}`} className="card-meta-item">
                                    {i > 0 && <span className="card-meta-sep">·</span>}
                                    {bit}
                                  </span>
                                ))}
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
                    </AudioMeters>
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
