"use client";

import { useBodyScrollLock } from "@/hooks/useBodyScrollLock";
import type { PresetEditorKind } from "@/lib/presetRoles";

type Props = {
  open: boolean;
  kind: PresetEditorKind;
  onClose: () => void;
};

/** Plain-language help for proxy / HQ preset builders. */
export default function PresetHelpModal({ open, kind, onClose }: Props) {
  useBodyScrollLock(open);

  if (!open) return null;

  return (
    <div className="modal-backdrop preset-help-backdrop" onClick={onClose} role="presentation">
      <div
        className="modal-panel preset-help-modal"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-label="Preset help"
      >
        <div className="modal-header">
          <h2>{kind === "proxy" ? "Proxy presets — quick guide" : "HQ presets — quick guide"}</h2>
          <button type="button" className="modal-close" onClick={onClose} aria-label="Close">
            ×
          </button>
        </div>

        <div className="preset-help-body">
          {kind === "proxy" ? (
            <>
              <p>
                Proxy is the light encode used for live preview, SRT, and (if you want) a small
                proxy file. It is <em>not</em> the finishing / Avid mezz.
              </p>
              <p>
                Start with <strong>12 Mbit</strong> unless you know your network needs something
                lighter or heavier. Encoder speed “p4” is a good default.
              </p>
            </>
          ) : (
            <>
              <p>
                HQ presets build the <strong>recording file</strong> (mezz). Live preview and SRT
                still use the channel’s Proxy preset — these two jobs stay separate on purpose.
              </p>

              <h3>DNxHD — pick a class, not a bitrate</h3>
              <p>
                You choose quality class. We set the real Mbps from the <em>live</em> signal. Only{" "}
                <strong>1080i50</strong> and <strong>1080p50</strong> are supported. On the channel
                card / preview you will see the resolved name (e.g. <strong>DNxHD 185</strong>), not
                a fixed “185” from the preset store.
              </p>
              <ul className="preset-help-list">
                <li>
                  <strong>HQ (8-bit)</strong> — the normal pick for most feeds.
                </li>
                <li>
                  <strong>SQ (8-bit)</strong> — smaller files when disk or bandwidth is tight.
                </li>
                <li>
                  <strong>HQX (10-bit)</strong> — only if the <em>source</em> is truly 10-bit. Same
                  family bitrate as HQ (185x / 365x), but real 10-bit colour — we will not invent
                  10-bit from an 8-bit signal.
                </li>
              </ul>

              <table className="preset-help-table">
                <thead>
                  <tr>
                    <th>Signal</th>
                    <th>SQ</th>
                    <th>HQ</th>
                    <th>HQX</th>
                  </tr>
                </thead>
                <tbody>
                  <tr>
                    <td>1080i50</td>
                    <td>120</td>
                    <td>185</td>
                    <td>185x</td>
                  </tr>
                  <tr>
                    <td>1080p50</td>
                    <td>240</td>
                    <td>365</td>
                    <td>365x</td>
                  </tr>
                </tbody>
              </table>
              <p className="preset-help-note">
                Example: 1080i50 + HQ → <strong>DNxHD 185</strong>. 1080p50 + HQ →{" "}
                <strong>DNxHD 365</strong>.
              </p>
              <p className="preset-help-note">
                MediaInfo / Avid often label the HQ family as “220” (NTSC naming). For 1080i50 that
                is still our <strong>185</strong> operating point — trust the ROC label and the
                table above.
              </p>

              <h3>10-bit source + 8-bit class?</h3>
              <p>
                If the feed is 10-bit and you pick SQ/HQ, we <strong>downconvert to 8-bit</strong>{" "}
                (Y42B) and keep recording. No error — but you lose the extra colour depth. Use{" "}
                <strong>HQX</strong> when you want to keep 10-bit. The reverse (8-bit source + HQX)
                is blocked.
              </p>

              <h3>XAVC</h3>
              <p>
                Our XAVC option is an open-source Intra approximation — fine for many edit
                workflows, not a certified Sony Class 100 deliverable.
              </p>
            </>
          )}
        </div>

        <div className="presets-form-actions">
          <button type="button" className="global-rec-btn" onClick={onClose}>
            Got it
          </button>
        </div>
      </div>
    </div>
  );
}
