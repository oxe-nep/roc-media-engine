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
import DecodeSettingsModal from "@/components/DecodeSettingsModal";

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

function prettyCodec(raw?: string): string {
  if (!raw?.trim()) return "";
  const c = raw.trim().toLowerCase();
  const map: Record<string, string> = {
    h264: "H.264",
    avc: "H.264",
    hevc: "H.265",
    h265: "H.265",
    aac: "AAC",
    mp3: "MP3",
    opus: "Opus",
    pcm_s16le: "PCM",
    pcm_s24le: "PCM",
    pcm_s32le: "PCM",
    pcm_f32le: "PCM",
    dnxhd: "DNxHD",
    prores: "ProRes",
  };
  return map[c] || raw.toUpperCase();
}

/** e.g. `4×AAC 2ch` or `AAC 8ch` or `AAC 2ch`. */
function audioLayoutLabel(c: PlayoutClient): string {
  const codec = prettyCodec(c.audio_codec) || "Audio";
  const tracks = Math.max(0, c.audio_tracks ?? 0);
  const ch = Math.max(0, c.audio_channels ?? 0);
  if (tracks > 1) {
    const per = ch > 0 ? Math.max(1, Math.round(ch / tracks)) : 2;
    return `${tracks}×${codec} ${per}ch`;
  }
  if (ch > 0) return `${codec} ${ch}ch`;
  if (c.audio_codec) return codec;
  return "";
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

function cardMetaLine(c: PlayoutClient): string {
  const bits: string[] = [];
  if (c.source === "file") {
    const v = prettyCodec(c.video_codec);
    if (v) bits.push(v);
    const a = audioLayoutLabel(c);
    if (a) bits.push(a);
    bits.push(formatDisplay(c.format_code));
  } else {
    bits.push(c.mode === "listener" ? "Listener" : "Caller");
    bits.push(formatDisplay(c.format_code));
    if ((c.latency_ms ?? 0) > 0) bits.push(`${c.latency_ms} ms`);
  }
  return bits.join(" · ");
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
            const playing = on && !paused;
            const isFile = c.source === "file";
            const title = cardTitle(c);
            const meta = cardMetaLine(c);
            const actionError = cardError[c.id];
            const engineError = c.error?.trim() || "";
            const showError = actionError || engineError;
            return (
              <div key={c.id} className={`card-panel decode-card ${c.status}`}>
                <div className="decode-row">
                  <span className={`card-channel-num ${c.status}`} title={`Output ${c.id}`}>
                    {c.id}
                  </span>
                  <div className="decode-body">
                    <div className="decode-head">
                      <span className="card-name" title={title}>
                        {title}
                      </span>
                      <div className="card-meta" title={meta}>
                        <span className="card-meta-item">{meta}</span>
                      </div>
                    </div>

                    <div className="decode-toolbar">
                      {isFile ? (
                        <div className="transport">
                          <button
                            type="button"
                            className={`ctrl-btn${playing ? " on" : ""}`}
                            disabled={busy[c.id] || playing || (!c.file_id && !paused)}
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
                          <button
                            type="button"
                            className={`ctrl-btn${paused ? " on" : ""}`}
                            disabled={busy[c.id] || !playing}
                            onClick={() => withBusy(c.id, async () => pausePlayout(c.id))}
                            title="Pause"
                          >
                            PAUSE
                          </button>
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
