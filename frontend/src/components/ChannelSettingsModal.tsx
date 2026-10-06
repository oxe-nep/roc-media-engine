"use client";

import { useEffect, useRef, useState } from "react";
import {
  fetchSrt,
  fetchStreamLogs,
  setEncodePreset,
  setRecordPreset,
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
import { formatPresetLabel, isProresCodec, isProxyPreset, isRecPreset } from "@/lib/presetRoles";

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
  const [proxyPreset, setProxyPreset] = useState("");
  const [recPreset, setRecPreset] = useState("");
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
  const [schedArmProxy, setSchedArmProxy] = useState(false);
  const [schedArmHq, setSchedArmHq] = useState(false);
  const [schedBusy, setSchedBusy] = useState(false);
  const [encodeBusy, setEncodeBusy] = useState(false);
  const [logs, setLogs] = useState<string[]>([]);
  const [logsOpen, setLogsOpen] = useState(false);
  const logBoxRef = useRef<HTMLPreElement>(null);
  const baselineRef = useRef({ name: "", category: "", proxyPreset: "", recPreset: "" });

  useBodyScrollLock(open);

  const isRunning = stream?.status === "running";
  const captureOn = stream ? isCaptureOn(stream.status) : false;
  const isRecording = recording?.status === "recording";
  const channelId = stream?.id;
  const srtStreaming = srt?.status === "streaming";
  const proxyPresets = presets.filter(isProxyPreset);
  const recPresets = presets.filter(isRecPreset);

  // Hydrate once when the modal opens for a channel — not on every 1s poll.
  useEffect(() => {
    if (!open || channelId == null || !stream) return;
    const nextName = recording?.name || `ch${channelId}`;
    const nextCategory = recording?.category || "_unsorted";
    const nextProxy = stream.encode_preset || "";
    const nextRecRaw = stream.record_preset || stream.encode_preset || "";
    const hqIds = new Set(presets.filter(isRecPreset).map((p) => p.id));
    const nextRec =
      (nextRecRaw && hqIds.has(nextRecRaw) && nextRecRaw) ||
      presets.find(isRecPreset)?.id ||
      nextRecRaw;
    setName(nextName);
    setCategory(nextCategory);
    setProxyPreset(nextProxy);
    setRecPreset(nextRec);
    baselineRef.current = {
      name: nextName,
      category: nextCategory,
      proxyPreset: nextProxy,
      recPreset: nextRec,
    };
    setError(null);
    setSrtPassphrase("");
    setSrtPassDirty(false);
    setLogsOpen(false);
    setLogs([]);
    if (recording?.schedule) {
      setSchedStart(toLocalInput(recording.schedule.start_at));
      setSchedStop(toLocalInput(recording.schedule.stop_at));
      setSchedArmProxy(recording.schedule.arm_proxy ?? false);
      setSchedArmHq(recording.schedule.arm_hq ?? true);
    } else {
      const d = defaultScheduleInputs();
      setSchedStart(d.start);
      setSchedStop(d.stop);
      setSchedArmProxy(false);
      setSchedArmHq(false);
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
      if (e.key === "Escape" && !busy && !srtBusy && !schedBusy && !encodeBusy) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose, busy, srtBusy, schedBusy, encodeBusy]);

  if (!open || !stream) return null;

  const toggleEncode = async () => {
    setEncodeBusy(true);
    setError(null);
    try {
      if (captureOn) {
        if (isRecording) {
          setError("Stop recording first");
          return;
        }
        if (
          !window.confirm(
            `Stop encode on channel ${stream.id}? Preview, REC and SRT will stop until encode is started again.`,
          )
        ) {
          return;
        }
        await stopStream(stream.id);
      } else {
        await startStream(stream.id);
      }
      onSaved();
    } catch (e) {
      setError(String(e));
    } finally {
      setEncodeBusy(false);
    }
  };

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
      const nameChanged = Boolean(cleanName && cleanName !== baseline.name);
      const categoryChanged = Boolean(category && category !== baseline.category);
      if (nameChanged) {
        await setRecordingName(stream.id, cleanName);
      }
      if (categoryChanged) {
        await setRecordingCategory(stream.id, category);
      }
      const proxyChanged = Boolean(proxyPreset && proxyPreset !== baseline.proxyPreset);
      const recChanged = Boolean(recPreset && recPreset !== baseline.recPreset);
      if (proxyChanged) {
        await setEncodePreset(stream.id, proxyPreset);
      }
      if (recChanged) {
        await setRecordPreset(stream.id, recPreset);
      }

      // Name / category / HQ preset are metadata only — never bounce capture.
      // Proxy preset: backend relaunches when capture is already running.
      // If capture is off and proxy changed, start once so the new preset is live.
      if (proxyChanged && !captureOn) {
        await startStream(stream.id);
      }

      baselineRef.current = {
        name: cleanName || baseline.name,
        category: category || baseline.category,
        proxyPreset: proxyChanged ? proxyPreset : baseline.proxyPreset,
        recPreset: recChanged ? recPreset : baseline.recPreset,
      };
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

  const persistSchedule = async (armProxy: boolean, armHq: boolean) => {
    const start = new Date(schedStart);
    const stop = new Date(schedStop);
    if (Number.isNaN(start.getTime()) || Number.isNaN(stop.getTime())) {
      throw new Error("Invalid schedule time");
    }
    if (!(stop.getTime() > start.getTime())) {
      throw new Error("Stop must be after start");
    }
    await setRecordingSchedule(stream.id, start.toISOString(), stop.toISOString(), {
      arm_proxy: armProxy,
      arm_hq: armHq,
    });
    onSaved();
  };

  const clearScheduleState = () => {
    const d = defaultScheduleInputs();
    setSchedStart(d.start);
    setSchedStop(d.stop);
    setSchedArmProxy(false);
    setSchedArmHq(false);
  };

  const toggleScheduleArm = async (role: "proxy" | "hq") => {
    const nextProxy = role === "proxy" ? !schedArmProxy : schedArmProxy;
    const nextHq = role === "hq" ? !schedArmHq : schedArmHq;
    setSchedBusy(true);
    setError(null);
    try {
      if (role === "proxy") setSchedArmProxy(nextProxy);
      else setSchedArmHq(nextHq);

      // Both disarmed = clear schedule (same as Clear).
      if (!nextProxy && !nextHq) {
        await clearRecordingSchedule(stream.id);
        clearScheduleState();
        onSaved();
        return;
      }

      await persistSchedule(nextProxy, nextHq);
    } catch (e) {
      // Revert optimistic toggle on failure.
      if (role === "proxy") setSchedArmProxy(!nextProxy);
      else setSchedArmHq(!nextHq);
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
      clearScheduleState();
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
    <div className="modal-backdrop" onClick={() => !busy && !srtBusy && !schedBusy && !encodeBusy && onClose()} role="presentation">
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
            disabled={busy || srtBusy || schedBusy || encodeBusy}
          >
            ×
          </button>
        </div>

        {error && <div className="error-message">{error}</div>}

        {isRecording && (
          <div className="channel-settings-lock">Recording — settings locked.</div>
        )}

        <div className="channel-settings-encode">
          <div className="channel-settings-encode-meta">
            <span className="channel-settings-encode-label">Encode</span>
            <span className={`channel-settings-encode-status ${captureOn ? "on" : "off"}`}>
              {captureOn ? (isRunning ? "Running" : "Waiting") : "Stopped"}
            </span>
          </div>
          <button
            type="button"
            className={`stream-btn ${captureOn ? "streaming" : "idle"}`}
            onClick={() => void toggleEncode()}
            disabled={encodeBusy || busy || (captureOn && isRecording)}
            title={
              captureOn && isRecording
                ? "Stop recording first"
                : captureOn
                  ? "Stop encode capture"
                  : "Start encode capture"
            }
          >
            {encodeBusy ? "…" : captureOn ? "STOP ENCODE" : "START ENCODE"}
          </button>
        </div>

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
            <span>Proxy preset</span>
            <select
              value={proxyPreset}
              onChange={(e) => setProxyPreset(e.target.value)}
              disabled={busy || isRecording || proxyPresets.length === 0}
            >
              {proxyPresets.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.label}
                </option>
              ))}
            </select>
          </label>

          <label className="presets-field">
            <span>HQ preset</span>
            <select
              value={recPreset}
              onChange={(e) => setRecPreset(e.target.value)}
              disabled={busy || isRecording || recPresets.length === 0}
            >
              {recPresets.map((p) => (
                <option key={p.id} value={p.id}>
                  {formatPresetLabel(p)}
                </option>
              ))}
            </select>
            {recPresets.find((p) => p.id === recPreset && isProresCodec(p.video_codec)) ? (
              <span className="channel-settings-hint">
                ProRes is experimental on this host (CPU encode) — prefer DNxHD for reliable mezz.
              </span>
            ) : null}
            {stream?.mezz_label ? (
              <span className="channel-settings-hint">
                Live signal → {stream.mezz_label}
                {stream.bit_depth === 8 || stream.bit_depth === 10
                  ? ` · source ${stream.bit_depth}-bit`
                  : ""}
              </span>
            ) : null}
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
            Set start/stop, then arm PROXY and/or HQ — arming saves the schedule. If there is no
            signal at start, recording waits until signal or stop.
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
            <div className="channel-settings-arms" role="group" aria-label="Schedule arms">
              <button
                type="button"
                className={`schedule-arm-btn ${schedArmHq ? "armed" : ""}`}
                onClick={() => toggleScheduleArm("hq")}
                disabled={schedBusy}
                aria-pressed={schedArmHq}
              >
                ARM HQ REC
              </button>
              <button
                type="button"
                className={`schedule-arm-btn ${schedArmProxy ? "armed" : ""}`}
                onClick={() => toggleScheduleArm("proxy")}
                disabled={schedBusy}
                aria-pressed={schedArmProxy}
              >
                ARM PROXY REC
              </button>
            </div>
          </div>
          <div className="channel-settings-actions">
            <button type="button" className="badge" onClick={clearSchedule} disabled={schedBusy || !recording?.schedule}>
              {schedBusy ? "…" : "Clear"}
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
