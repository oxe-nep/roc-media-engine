"use client";

import { useEffect, useState } from "react";
import {
  fetchEncodePresets,
  fetchTcLoop,
  setEncodePreset,
  setRecordingName,
  updateTcLoop,
  type EncodePreset,
  type TcLoopPosition,
  type TcLoopSource,
} from "@/lib/api";
import { isProxyPreset } from "@/lib/presetRoles";
import TcPositionPreview from "@/components/TcPositionPreview";
import { useBodyScrollLock } from "@/hooks/useBodyScrollLock";
import {
  defaultTcUdpPort,
  tcStatusLabel,
  tcStatusPillClass,
} from "@/lib/tcUi";

type Props = {
  open: boolean;
  channelId: number | null;
  /** Display / recording name for this channel. */
  channelName?: string;
  /** Current proxy encode preset id. */
  encodePreset?: string;
  onClose: () => void;
  onSaved: () => void;
};

const DEFAULT_X = 0.04;
const DEFAULT_Y = 0.04;
const DEFAULT_FONT = 48;

export default function TcSettingsModal({
  open,
  channelId,
  channelName,
  encodePreset,
  onClose,
  onSaved,
}: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [tcEnabled, setTcEnabled] = useState(false);
  const [tcStatus, setTcStatus] = useState<"off" | "running" | "restarting" | "error">("off");
  const [tcSource, setTcSource] = useState<TcLoopSource>("tod");
  const [tcUdpPort, setTcUdpPort] = useState(0);
  const [tcFontSize, setTcFontSize] = useState(DEFAULT_FONT);
  const [tcOpacity, setTcOpacity] = useState(0.9);
  const [tcPosition, setTcPosition] = useState<TcLoopPosition>("top_left");
  const [tcX, setTcX] = useState(DEFAULT_X);
  const [tcY, setTcY] = useState(DEFAULT_Y);
  const [tcError, setTcError] = useState("");
  const [tcApplyMsg, setTcApplyMsg] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [proxyPreset, setProxyPreset] = useState("");
  const [presets, setPresets] = useState<EncodePreset[]>([]);

  useBodyScrollLock(open);

  const tcOn = tcEnabled || tcStatus === "running" || tcStatus === "restarting";
  const tcEffectivePort = tcUdpPort > 0 ? tcUdpPort : defaultTcUdpPort(channelId ?? 0);
  const proxyPresets = presets.filter(isProxyPreset);

  useEffect(() => {
    if (!open || channelId == null) return;
    setError(null);
    setTcApplyMsg(null);
    setName((channelName || `ch${channelId}`).trim() || `ch${channelId}`);
    setProxyPreset(encodePreset || "");
    fetchEncodePresets()
      .then((list) => {
        setPresets(list);
        const proxies = list.filter(isProxyPreset);
        setProxyPreset((prev) => {
          const want = encodePreset || prev;
          if (want && proxies.some((p) => p.id === want)) return want;
          return proxies[0]?.id || want || "";
        });
      })
      .catch(() => {});
    fetchTcLoop(channelId)
      .then((tc) => {
        setTcEnabled(!!tc.enabled);
        setTcStatus(tc.status);
        setTcSource(tc.source === "external" ? "external" : "tod");
        setTcUdpPort(tc.udp_port || defaultTcUdpPort(channelId));
        setTcFontSize(tc.fontsize || DEFAULT_FONT);
        setTcOpacity(tc.opacity ?? 0.9);
        setTcPosition(tc.position || "top_left");
        setTcX(typeof tc.x === "number" ? tc.x : DEFAULT_X);
        setTcY(typeof tc.y === "number" ? tc.y : DEFAULT_Y);
        setTcError(tc.error || "");
      })
      .catch((e) => setError(String(e)));
  }, [open, channelId, channelName, encodePreset]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose, busy]);

  if (!open || channelId == null) return null;

  const applyTc = async (enabled: boolean) => {
    setBusy(true);
    setError(null);
    setTcApplyMsg(enabled ? "Starting…" : "Stopping…");
    try {
      const cleanName = name.trim() || `ch${channelId}`;
      await setRecordingName(channelId, cleanName);
      setName(cleanName);
      if (proxyPreset) {
        await setEncodePreset(channelId, proxyPreset);
      }
      const tc = await updateTcLoop(channelId, {
        enabled,
        source: tcSource,
        udp_port: tcSource === "external" ? tcEffectivePort : 0,
        fontsize: tcFontSize,
        opacity: tcOpacity,
        position: tcPosition,
        x: tcX,
        y: tcY,
      });
      setTcEnabled(!!tc.enabled);
      setTcStatus(tc.status);
      setTcSource(tc.source === "external" ? "external" : "tod");
      setTcUdpPort(tc.udp_port || defaultTcUdpPort(channelId));
      setTcFontSize(tc.fontsize || DEFAULT_FONT);
      setTcOpacity(tc.opacity ?? 0.9);
      setTcPosition(tc.position || "top_left");
      setTcX(typeof tc.x === "number" ? tc.x : tcX);
      setTcY(typeof tc.y === "number" ? tc.y : tcY);
      setTcError(tc.error || "");

      if (tc.enabled) {
        setTcApplyMsg("Waiting for signal…");
        for (let i = 0; i < 20; i++) {
          await new Promise((r) => setTimeout(r, 400));
          const latest = await fetchTcLoop(channelId);
          setTcStatus(latest.status);
          setTcError(latest.error || "");
          if (latest.status === "running") {
            setTcApplyMsg(null);
            break;
          }
          if (latest.status === "error") {
            setTcApplyMsg(null);
            setError(latest.error || "Failed to start");
            break;
          }
        }
      } else {
        setTcApplyMsg(null);
      }
      onSaved();
    } catch (e) {
      setTcApplyMsg(null);
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="modal-backdrop" onClick={() => !busy && onClose()} role="presentation">
      <div
        className="modal-panel channel-settings-modal"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-label={`TC burn-in channel ${channelId}`}
      >
        <div className="modal-header">
          <h2>
            <span className="input-badge decode">{channelId}</span>
            <span>TC</span>
          </h2>
          <button type="button" className="modal-close" onClick={onClose} aria-label="Close" disabled={busy}>
            ×
          </button>
        </div>

        {error && <div className="error-message">{error}</div>}
        {tcApplyMsg && <div className="channel-settings-note tc-apply-note">{tcApplyMsg}</div>}

        <div className={`channel-settings-srt channel-settings-tc${tcOn ? " enabled" : ""}`}>
          <div className="channel-settings-srt-head">
            <span className={tcStatusPillClass(tcStatus, tcEnabled)}>
              {tcStatusLabel(tcStatus, tcEnabled)}
            </span>
          </div>
          <div className="channel-settings-form">
            <div className="tc-settings-body">
              <div className="tc-settings-row">
                <div className="tc-settings-fields">
                  <label className="presets-field">
                    <span>Name</span>
                    <input
                      type="text"
                      value={name}
                      onChange={(e) => setName(e.target.value)}
                      disabled={busy}
                      placeholder={`ch${channelId}`}
                      autoComplete="off"
                    />
                  </label>
                  <label className="presets-field">
                    <span>Proxy preset</span>
                    <select
                      value={proxyPreset}
                      onChange={(e) => setProxyPreset(e.target.value)}
                      disabled={busy || proxyPresets.length === 0}
                    >
                      {proxyPresets.length === 0 && <option value="">No proxy presets</option>}
                      {proxyPresets.map((p) => (
                        <option key={p.id} value={p.id}>
                          {p.label || p.id}
                        </option>
                      ))}
                    </select>
                  </label>
                  <div className="presets-field">
                    <span>Timecode source</span>
                    <div className="tc-source-options">
                      <label className={`workflow-option${tcSource === "tod" ? " active" : ""}`}>
                        <input
                          type="radio"
                          name={`tc-source-${channelId}`}
                          checked={tcSource === "tod"}
                          disabled={busy}
                          onChange={() => setTcSource("tod")}
                        />
                        <span className="workflow-option-text">
                          <strong>Time Of Day</strong>
                          <span className="workflow-option-hint">Host clock · HH:MM:SS</span>
                        </span>
                      </label>
                      <label className={`workflow-option${tcSource === "external" ? " active" : ""}`}>
                        <input
                          type="radio"
                          name={`tc-source-${channelId}`}
                          checked={tcSource === "external"}
                          disabled={busy}
                          onChange={() => setTcSource("external")}
                        />
                        <span className="workflow-option-text">
                          <strong>UDP Input</strong>
                          <span className="workflow-option-hint">External · HH:MM:SS</span>
                        </span>
                      </label>
                    </div>
                  </div>
                  {tcSource === "external" && (
                    <label className="presets-field">
                      <span>UDP port</span>
                      <input
                        type="number"
                        min={1}
                        max={65535}
                        value={tcEffectivePort}
                        onChange={(e) =>
                          setTcUdpPort(Number(e.target.value) || defaultTcUdpPort(channelId))
                        }
                        disabled={busy}
                      />
                    </label>
                  )}
                  <label className="presets-field">
                    <span>Font size ({tcFontSize} px on 1080)</span>
                    <input
                      type="range"
                      min={16}
                      max={120}
                      step={1}
                      value={tcFontSize}
                      onChange={(e) => setTcFontSize(Number(e.target.value) || DEFAULT_FONT)}
                      disabled={busy}
                    />
                  </label>
                  <label className="presets-field">
                    <span>Opacity ({Math.round(tcOpacity * 100)}%)</span>
                    <input
                      type="range"
                      min={0.2}
                      max={1}
                      step={0.05}
                      value={tcOpacity}
                      onChange={(e) => setTcOpacity(Number(e.target.value))}
                      disabled={busy}
                    />
                  </label>
                  <div className="presets-field">
                    <span>
                      Position ({Math.round(tcX * 100)}%, {Math.round(tcY * 100)}%)
                    </span>
                    <div className="tc-xy-sliders">
                      <label className="tc-xy-slider">
                        <span>X</span>
                        <input
                          type="range"
                          min={0}
                          max={100}
                          step={1}
                          value={Math.round(tcX * 100)}
                          disabled={busy}
                          onChange={(e) => setTcX(Number(e.target.value) / 100)}
                        />
                      </label>
                      <label className="tc-xy-slider">
                        <span>Y</span>
                        <input
                          type="range"
                          min={0}
                          max={100}
                          step={1}
                          value={Math.round(tcY * 100)}
                          disabled={busy}
                          onChange={(e) => setTcY(Number(e.target.value) / 100)}
                        />
                      </label>
                    </div>
                  </div>
                </div>
                <TcPositionPreview
                  x={tcX}
                  y={tcY}
                  fontsize={tcFontSize}
                  opacity={tcOpacity}
                  disabled={busy}
                  onMove={(nx, ny) => {
                    setTcX(nx);
                    setTcY(ny);
                  }}
                  onSnap={(pos) => setTcPosition(pos)}
                />
              </div>
            </div>
            {tcError && tcStatus === "error" && (
              <div className="channel-settings-lock tc-error-note">{tcError}</div>
            )}
          </div>
          <div className="channel-settings-actions">
            {tcOn ? (
              <>
                <button
                  type="button"
                  className="global-rec-btn tc-apply-on"
                  onClick={() => applyTc(true)}
                  disabled={busy}
                >
                  {busy ? "…" : "Update"}
                </button>
                <button type="button" className="tc-stop-btn" onClick={() => applyTc(false)} disabled={busy}>
                  {busy ? "…" : "Stop"}
                </button>
              </>
            ) : (
              <button
                type="button"
                className="global-rec-btn tc-apply-on"
                onClick={() => applyTc(true)}
                disabled={busy}
              >
                {busy ? "…" : "Start"}
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
