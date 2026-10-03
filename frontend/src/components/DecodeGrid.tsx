"use client";

import { useState } from "react";
import {
  isPlayoutOn,
  isPlayoutPaused,
  pausePlayout,
  resumePlayout,
  startPlayout,
  stopPlayout,
  updatePlayoutClient,
  type PlayoutClient,
} from "@/lib/api";
import { showDecodeCard } from "@/lib/workflow";
import { sortByChannelId } from "@/lib/sortChannels";
import { useWorkflows } from "@/hooks/useWorkflows";
import { useDashboard } from "@/hooks/useDashboard";
import HlsPreview from "@/components/HlsPreview";
import AudioMeters from "@/components/AudioMeters";
import ListenButton from "@/components/ListenButton";
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
  const map: Record<string, string> = {
    Hi50: "1080i50",
    Hp50: "1080p50",
    Hi25: "1080i25",
    Hp25: "1080p25",
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

export default function DecodeGrid() {
  const { loading, playout: clients } = useDashboard();
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<Record<number, boolean>>({});
  const [listenPair, setListenPair] = useState<Record<number, number | null>>({});
  const [settingsId, setSettingsId] = useState<number | null>(null);
  const { workflows } = useWorkflows();

  const withBusy = async (id: number, fn: () => Promise<unknown>) => {
    setBusy((b) => ({ ...b, [id]: true }));
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy((b) => ({ ...b, [id]: false }));
    }
  };

  const toggleLoop = async (c: PlayoutClient) => {
    const next = !c.loop;
    try {
      await updatePlayoutClient(c.id, { loop: next });
    } catch (e) {
      setError(String(e));
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

      {error && (
        <div className="error-message">
          {error}
          <button type="button" className="error-dismiss" onClick={() => setError(null)} aria-label="Dismiss">
            ×
          </button>
        </div>
      )}

      {loading && clients.length === 0 ? (
        <div className="loading">
          <span>…</span>
        </div>
      ) : (
        <div className="cards-grid">
          {visibleClients.map((c) => {
            const on = isPlayoutOn(c.status);
            const paused = isPlayoutPaused(c.status);
            const listenAt = listenPair[c.id] ?? null;
            const isFile = c.source === "file";
            const title = cardTitle(c);
            const previewOn = isFile ? on && !!c.sending : on;
            const previewKey = `${c.id}-${c.file_id ?? ""}-${c.sending ? "live" : "idle"}`;
            return (
              <div key={c.id} className={`card-panel ${c.status}`}>
                <div className="card-stage">
                  <AudioMeters channelId={c.id} bus="playout" channels={2}>
                  <div className="card-thumb">
                    <HlsPreview
                      active={previewOn}
                      listenPair={listenAt}
                      playlistPath={`/hls/playout/${c.id}/preview.m3u8?s=${encodeURIComponent(previewKey)}`}
                      sessionKey={previewKey}
                    />
                    {on && isFile && (c.duration_sec ?? 0) > 0 && (
                      <div className="thumb-badges">
                        <div className="stream-badge waiting" title="Elapsed / remaining">
                          {formatClock(c.elapsed_sec)} / −{formatClock(c.remain_sec)}
                        </div>
                      </div>
                    )}
                  </div>
                  </AudioMeters>
                </div>

                <div className="card-footer">
                  <div className="card-top">
                    <div className="card-identity">
                      <span className={`card-channel-num ${c.status}`} title={`Output ${c.id}`}>
                        {c.id}
                      </span>
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
                      {on && (
                        <ListenButton
                          pair={listenAt}
                          stereoOnly
                          onChange={(p) => setListenPair((prev) => ({ ...prev, [c.id]: p }))}
                        />
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
