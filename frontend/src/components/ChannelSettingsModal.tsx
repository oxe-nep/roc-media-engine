"use client";

import { useEffect, useRef, useState } from "react";
import {
  fetchSrt,
  fetchStreamLogs,
  setEncodePreset,
  setRecordingCategory,
  setRecordingName,
  setRecordingSchedule,
  clearRecordingSchedule,
  startSrt,
  startStream,
  stopSrt,
  stopStream,
  updateSrt,
  type EncodePreset,
  type LibraryCategory,
  type RecordingInfo,
  type SrtInfo,
  type Stream,
  isCaptureOn,
} from "@/lib/api";
import { useBodyScrollLock } from "@/hooks/useBodyScrollLock";

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

function toLocalInput(iso?: string, fallback?: Date): string {
  const d = iso ? new Date(iso) : fallback ?? new Date();
  if (Number.isNaN(d.getTime())) return "";
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}T${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}

function defaultScheduleInputs(): { start: string; stop: string } {
  const start = new Date();
  start.setSeconds(0, 0);
  start.setMinutes(start.getMinutes() + 5);
  const stop = new Date(start.getTime() + 60 * 60 * 1000);
  return { start: toLocalInput(undefined, start), stop: toLocalInput(undefined, stop) };
}

type Props = {
  open: boolean;
  stream: Stream | null;
  recording: RecordingInfo | null;
  presets: EncodePreset[];
  categories: LibraryCategory[];
  onClose: () => void;
  onSaved: () => void;
};

export default function ChannelSettingsModal({
  open,
  stream,
  recording,
  presets,
  categories,
  onClose,
  onSaved,
}: Props) {
  const [name, setName] = useState("");
  const [category, setCategory] = useState("_unsorted");
  const [preset, setPreset] = useState("");
  const [busy, setBusy] = useState(false);
  const [srtBusy, setSrtBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [srt, setSrt] = useState<SrtInfo | null>(null);
  const [srtMode, setSrtMode] = useState<"listener" | "caller">("listener");
  const [srtPort, setSrtPort] = useState(9101);
  const [srtTarget, setSrtTarget] = useState("");
  const [srtLatency, setSrtLatency] = useState(120);
  const [srtPassphrase, setSrtPassphrase] = useState("");
  const [srtPassDirty, setSrtPassDirty] = useState(false);
  const [schedStart, setSchedStart] = useState("");
  const [schedStop, setSchedStop] = useState("");
  const [schedBusy, setSchedBusy] = useState(false);
  const [logs, setLogs] = useState<string[]>([]);
  const [logsOpen, setLogsOpen] = useState(false);
  const logBoxRef = useRef<HTMLPreElement>(null);
  const baselineRef = useRef({ name: "", category: "", preset: "" });

  useBodyScrollLock(open);

  const isRunning = stream?.status === "running";
  const captureOn = stream ? isCaptureOn(stream.status) : false;
  const isRecording = recording?.status === "recording";
  const channelId = stream?.id;
  const srtStreaming = srt?.status === "streaming";

  // Hydrate once when the modal opens for a channel — not on every 1s poll.
  useEffect(() => {
    if (!open || channelId == null || !stream) return;
    const nextName = recording?.name || `ch${channelId}`;
    const nextCategory = recording?.category || "_unsorted";
    const nextPreset = stream.encode_preset || "";
    setName(nextName);
    setCategory(nextCategory);
    setPreset(nextPreset);
    baselineRef.current = {
      name: nextName,
      category: nextCategory,
      preset: nextPreset,
    };
    setError(null);
    setSrtPassphrase("");
    setSrtPassDirty(false);
    setLogsOpen(false);
    setLogs([]);
    if (recording?.schedule) {
      setSchedStart(toLocalInput(recording.schedule.start_at));
      setSchedStop(toLocalInput(recording.schedule.stop_at));
    } else {
      const d = defaultScheduleInputs();
      setSchedStart(d.start);
      setSchedStop(d.stop);
    }

    fetchSrt(channelId)
      .then((info) => {
        setSrt(info);
        setSrtMode(info.mode || "listener");
        setSrtPort(info.port || 9100 + channelId);
        setSrtTarget(info.target || "");
        setSrtLatency(info.latency_ms || 120);
      })
      .catch((e) => setError(String(e)));
    // intentionally only when open / channel changes
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, channelId]);

  useEffect(() => {
    if (!open || channelId == null) return;
    const tick = () => {
      fetchSrt(channelId)
        .then(setSrt)
        .catch(() => {});
    };
    const interval = setInterval(tick, 2000);
    return () => clearInterval(interval);
  }, [open, channelId]);

  useEffect(() => {
    if (!open || channelId == null || !logsOpen) return;
    let alive = true;
    const tick = async () => {
      try {
        const lines = await fetchStreamLogs(channelId);
        if (alive) setLogs(lines);
      } catch {
        // ignore transient log fetch errors
      }
    };
    tick();
    const interval = setInterval(tick, 1500);
    return () => {
      alive = false;
      clearInterval(interval);
    };
  }, [open, channelId, logsOpen]);

  useEffect(() => {
    if (!logsOpen || !logBoxRef.current) return;
    logBoxRef.current.scrollTop = logBoxRef.current.scrollHeight;
  }, [logs, logsOpen]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy && !srtBusy && !schedBusy) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose, busy, srtBusy]);

  if (!open || !stream) return null;

  const apply = async () => {
    if (isRecording) {
      setError("Stop recording first");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const baseline = baselineRef.current;
      const cleanName = name.trim();
      if (cleanName && cleanName !== baseline.name) {
        await setRecordingName(stream.id, cleanName);
      }
      if (category && category !== baseline.category) {
        await setRecordingCategory(stream.id, category);
      }
      const presetChanged = Boolean(preset && preset !== baseline.preset);
      if (presetChanged) {
        await setEncodePreset(stream.id, preset);
      }

      // Backend restarts capture (and restores SRT) when the preset changes on
      // a live channel. Otherwise bounce capture so a stopped channel starts
      // (e.g. after TC → Encode) and name-only saves keep the previous behavior.
      if (!(presetChanged && captureOn)) {
        try {
          await stopSrt(stream.id);
        } catch {
          // ignore if SRT was not running
        }
        if (captureOn) {
          try {
            await stopStream(stream.id);
          } catch {
            // already stopped
          }
        }
        await startStream(stream.id);
      }

      onSaved();
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const saveSrt = async () => {
    setSrtBusy(true);
    setError(null);
    try {
      const body: Parameters<typeof updateSrt>[1] = {
        mode: srtMode,
        port: srtPort,
        target: srtTarget,
        latency_ms: srtLatency,
      };
      if (srtPassDirty) body.passphrase = srtPassphrase;
      const info = await updateSrt(stream.id, body);
      setSrt(info);
      setSrtPassDirty(false);
      if (!srtPassphrase) setSrtPassphrase("");
      onSaved();
    } catch (e) {
      setError(String(e));
    } finally {
      setSrtBusy(false);
    }
  };

  const toggleSrt = async () => {
    setSrtBusy(true);
    setError(null);
    try {
      if (srtStreaming) {
        setSrt(await stopSrt(stream.id));
      } else {
        // Persist current form values before start.
        const body: Parameters<typeof updateSrt>[1] = {
          mode: srtMode,
          port: srtPort,
          target: srtTarget,
          latency_ms: srtLatency,
        };
        if (srtPassDirty) body.passphrase = srtPassphrase;
        await updateSrt(stream.id, body);
        setSrtPassDirty(false);
        setSrt(await startSrt(stream.id));
      }
      onSaved();
    } catch (e) {
      setError(String(e));
    } finally {
      setSrtBusy(false);
    }
  };

  const saveSchedule = async () => {
    setSchedBusy(true);
    setError(null);
    try {
      const start = new Date(schedStart);
      const stop = new Date(schedStop);
      if (Number.isNaN(start.getTime()) || Number.isNaN(stop.getTime())) {
        throw new Error("Invalid schedule time");
      }
      if (!(stop.getTime() > start.getTime())) {
        throw new Error("Stop must be after start");
      }
      await setRecordingSchedule(stream.id, start.toISOString(), stop.toISOString());
      onSaved();
    } catch (e) {
      setError(String(e));
    } finally {
      setSchedBusy(false);
    }
  };

  const clearSchedule = async () => {
    setSchedBusy(true);
    setError(null);
    try {
      await clearRecordingSchedule(stream.id);
      const d = defaultScheduleInputs();
      setSchedStart(d.start);
      setSchedStop(d.stop);
      onSaved();
    } catch (e) {
      setError(String(e));
    } finally {
      setSchedBusy(false);
    }
  };

  const copyUrl = async () => {
    if (!srt?.publish_url) return;
    try {
      await navigator.clipboard.writeText(srt.publish_url);
    } catch {
      setError("Could not copy URL");
    }
  };

  return (
    <div className="modal-backdrop" onClick={() => !busy && !srtBusy && !schedBusy && onClose()} role="presentation">
      <div
        className="modal-panel channel-settings-modal"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-label={`Settings for ${stream.name}`}
      >
        <div className="modal-header">
          <h2>
            <span className="input-badge">{stream.id}</span>
            <span>{name.trim() || `ch${stream.id}`}</span>
          </h2>
          <button
            type="button"
            className="modal-close"
            onClick={onClose}
            aria-label="Close"
            disabled={busy || srtBusy || schedBusy}
          >
            ×
          </button>
        </div>

        {error && <div className="error-message">{error}</div>}

        {isRecording && (
          <div className="channel-settings-lock">Recording — settings locked.</div>
        )}

        <div className="channel-settings-form">
          <label className="presets-field">
            <span>Name</span>
            <input
              value={name}
              onChange={(e) => setName(e.target.value)}
              disabled={busy || isRecording}
              placeholder={`ch${stream.id}`}
            />
          </label>

          <label className="presets-field">
            <span>Category</span>
            <select
              value={category}
              onChange={(e) => setCategory(e.target.value)}
              disabled={busy || isRecording || categories.length === 0}
            >
              {categories.map((c) => (
                <option key={c.name} value={c.name}>
                  {c.name === "_unsorted" ? "Unsorted" : c.name}
                </option>
              ))}
            </select>
          </label>

          <label className="presets-field">
            <span>Preset</span>
            <select
              value={preset}
              onChange={(e) => setPreset(e.target.value)}
              disabled={busy || isRecording || presets.length === 0}
            >
              {presets.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.label}
                </option>
              ))}
            </select>
          </label>
        </div>

        <div className="channel-settings-actions">
          <button
            type="button"
            className="global-rec-btn"
            onClick={apply}
            disabled={busy || isRecording}
            title={isRecording ? "Stop recording first" : captureOn ? "Restarts capture" : undefined}
          >
            {busy ? "…" : "Apply"}
          </button>
        </div>

        <div className="channel-settings-srt">
          <div className="channel-settings-srt-head">
            <h3>Schedule</h3>
            <span className={`srt-pill ${recording?.schedule ? "on" : ""}`}>
              {recording?.schedule
                ? recording.schedule.phase === "waiting"
                  ? "WAITING"
                  : recording.schedule.phase === "active"
                    ? "ON"
                    : "SET"
                : "OFF"}
            </span>
          </div>
          <p className="channel-settings-hint">
            One-shot record window. If there is no signal at start, recording waits until signal or stop.
          </p>
          <div className="channel-settings-form">
            <label className="presets-field">
              <span>Start</span>
              <input
                type="datetime-local"
                value={schedStart}
                onChange={(e) => setSchedStart(e.target.value)}
                disabled={schedBusy}
              />
            </label>
            <label className="presets-field">
              <span>Stop</span>
              <input
                type="datetime-local"
                value={schedStop}
                onChange={(e) => setSchedStop(e.target.value)}
                disabled={schedBusy}
              />
            </label>
          </div>
          <div className="channel-settings-actions">
            <button type="button" className="badge" onClick={clearSchedule} disabled={schedBusy || !recording?.schedule}>
              {schedBusy ? "…" : "Clear"}
            </button>
            <button type="button" className="global-rec-btn" onClick={saveSchedule} disabled={schedBusy}>
              {schedBusy ? "…" : "Save"}
            </button>
          </div>
        </div>

        <div className="channel-settings-srt">
          <div className="channel-settings-srt-head">
            <h3>SRT</h3>
            <span className={`srt-pill ${srtStreaming ? "on" : ""}`}>
              {srtStreaming ? "ON AIR" : "OFF"}
            </span>
          </div>

          <div className="channel-settings-form">
            <label className="presets-field">
              <span>Mode</span>
              <select
                value={srtMode}
                onChange={(e) => setSrtMode(e.target.value as "listener" | "caller")}
                disabled={srtBusy || srtStreaming}
              >
                <option value="listener">Listener</option>
                <option value="caller">Caller</option>
              </select>
            </label>

            {srtMode === "listener" ? (
              <label className="presets-field">
                <span>Port</span>
                <input
                  type="number"
                  min={1}
                  max={65535}
                  value={srtPort}
                  onChange={(e) => setSrtPort(Number(e.target.value) || 0)}
                  disabled={srtBusy || srtStreaming}
                />
              </label>
            ) : (
              <label className="presets-field">
                <span>Target</span>
                <input
                  value={srtTarget}
                  onChange={(e) => setSrtTarget(e.target.value)}
                  disabled={srtBusy || srtStreaming}
                  placeholder="host:port or srt://…"
                />
              </label>
            )}

            <label className="presets-field">
              <span>Latency</span>
              <input
                type="number"
                min={20}
                max={8000}
                value={srtLatency}
                onChange={(e) => setSrtLatency(Number(e.target.value) || 120)}
                disabled={srtBusy || srtStreaming}
              />
            </label>

            <label className="presets-field">
              <span>Passphrase {srt?.has_passphrase && !srtPassDirty ? "(set)" : ""}</span>
              <input
                type="password"
                value={srtPassphrase}
                onChange={(e) => {
                  setSrtPassphrase(e.target.value);
                  setSrtPassDirty(true);
                }}
                disabled={srtBusy || srtStreaming}
                placeholder={srt?.has_passphrase ? "••••••••" : "optional"}
                autoComplete="new-password"
              />
            </label>
          </div>

          {srt?.publish_url && (
            <div className="srt-url-row">
              <code className="srt-url" title={srt.publish_url}>
                {srt.publish_url}
              </code>
              <button type="button" className="badge" onClick={copyUrl} disabled={!srt.publish_url}>
                Copy
              </button>
            </div>
          )}

          {srt?.error && <div className="error-bar">{srt.error}</div>}

          <div className="channel-settings-actions">
            <button
              type="button"
              className="badge"
              onClick={saveSrt}
              disabled={srtBusy || srtStreaming}
              title={srtStreaming ? "Stop SRT before changing settings" : "Save SRT settings"}
            >
              {srtBusy ? "…" : "Save"}
            </button>
            <button
              type="button"
              className={`global-rec-btn ${srtStreaming ? "recording" : ""}`}
              onClick={toggleSrt}
              disabled={srtBusy || (!isRunning && !srtStreaming)}
              title={!isRunning && !srtStreaming ? "Start channel first" : undefined}
            >
              {srtBusy ? "…" : srtStreaming ? "Stop" : "Start"}
            </button>
          </div>
        </div>

        <div className="channel-settings-logs">
          <div className="channel-settings-logs-head">
            <h3>Logs</h3>
            <button
              type="button"
              className="badge"
              onClick={() => setLogsOpen((v) => !v)}
            >
              {logsOpen ? "−" : "+"}
            </button>
          </div>
          {logsOpen && (
            <pre ref={logBoxRef} className="channel-settings-logbox">
              {logs.join("\n")}
            </pre>
          )}
        </div>
      </div>
    </div>
  );
}
