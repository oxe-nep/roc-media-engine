"use client";

import { useEffect, useState } from "react";
import { fetchTcLoop, updateTcLoop, type TcLoopPosition, type TcLoopSource } from "@/lib/api";
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
  onClose: () => void;
  onSaved: () => void;
};

export default function TcSettingsModal({ open, channelId, onClose, onSaved }: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [tcEnabled, setTcEnabled] = useState(false);
  const [tcStatus, setTcStatus] = useState<"off" | "running" | "restarting" | "error">("off");
  const [tcSource, setTcSource] = useState<TcLoopSource>("tod");
  const [tcUdpPort, setTcUdpPort] = useState(0);
  const [tcFontSize, setTcFontSize] = useState(96);
  const [tcOpacity, setTcOpacity] = useState(0.9);
  const [tcPosition, setTcPosition] = useState<TcLoopPosition>("top_left");
  const [tcError, setTcError] = useState("");
  const [tcApplyMsg, setTcApplyMsg] = useState<string | null>(null);

  useBodyScrollLock(open);

  const tcOn = tcEnabled || tcStatus === "running" || tcStatus === "restarting";
  const tcEffectivePort = tcUdpPort > 0 ? tcUdpPort : defaultTcUdpPort(channelId ?? 0);

  useEffect(() => {
    if (!open || channelId == null) return;
    setError(null);
    setTcApplyMsg(null);
    fetchTcLoop(channelId)
      .then((tc) => {
        setTcEnabled(!!tc.enabled);
        setTcStatus(tc.status);
        setTcSource(tc.source === "external" ? "external" : "tod");
        setTcUdpPort(tc.udp_port || defaultTcUdpPort(channelId));
        setTcFontSize(tc.fontsize || 96);
        setTcOpacity(tc.opacity ?? 0.9);
        setTcPosition(tc.position || "top_left");
        setTcError(tc.error || "");
      })
      .catch((e) => setError(String(e)));
  }, [open, channelId]);

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
      const tc = await updateTcLoop(channelId, {
        enabled,
        source: tcSource,
        udp_port: tcSource === "external" ? tcEffectivePort : 0,
        fontsize: tcFontSize,
        opacity: tcOpacity,
        position: tcPosition,
      });
      setTcEnabled(!!tc.enabled);
      setTcStatus(tc.status);
      setTcSource(tc.source === "external" ? "external" : "tod");
      setTcUdpPort(tc.udp_port || defaultTcUdpPort(channelId));
      setTcFontSize(tc.fontsize || 96);
      setTcOpacity(tc.opacity ?? 0.9);
      setTcPosition(tc.position || "top_left");
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
                          <span className="workflow-option-hint">Host clock</span>
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
                          <span className="workflow-option-hint">External LTC</span>
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
                    <span>Font size</span>
                    <input
                      type="number"
                      min={12}
                      max={200}
                      value={tcFontSize}
                      onChange={(e) => setTcFontSize(Number(e.target.value) || 96)}
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
                  <label className="presets-field">
                    <span>Position</span>
                    <select
                      value={tcPosition}
                      onChange={(e) => setTcPosition(e.target.value as TcLoopPosition)}
                      disabled={busy}
                    >
                      <option value="top_left">Top left</option>
                      <option value="top_right">Top right</option>
                      <option value="center">Center</option>
                      <option value="bottom_left">Bottom left</option>
                      <option value="bottom_right">Bottom right</option>
                    </select>
                  </label>
                </div>
                <TcPositionPreview position={tcPosition} fontsize={tcFontSize} opacity={tcOpacity} />
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
