"use client";

import { useState } from "react";
import {
  createCommentatorSession,
  revokeCommentatorSession,
} from "@/lib/api";
import {
  commentatorCardMeta,
  commentatorIsActive,
  commentatorNumClass,
  commentatorPanelClass,
  commentatorThumbLabel,
} from "@/lib/commentatorUi";
import { showCommentatorCard } from "@/lib/workflow";
import { sortByChannelId } from "@/lib/sortChannels";
import { useWorkflows } from "@/hooks/useWorkflows";
import { useDashboard } from "@/hooks/useDashboard";
import CommentatorSettingsModal from "@/components/CommentatorSettingsModal";

export default function CommentatorGrid() {
  const { loading, streams, commentatorById } = useDashboard();
  const { workflows } = useWorkflows();
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<Record<number, boolean>>({});
  const [settingsId, setSettingsId] = useState<number | null>(null);

  const visible = sortByChannelId(streams.filter((s) => showCommentatorCard(workflows, s.id)));

  const run = async (id: number, fn: () => Promise<void>) => {
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

  if (!loading && visible.length === 0) {
    return null;
  }

  return (
    <section className="io-section io-section-commentator">
      <div className="io-section-head">
        <h2 className="io-section-title">Remote Commentator</h2>
      </div>

      {error && (
        <div className="error-message">
          {error}
          <button type="button" className="error-dismiss" onClick={() => setError(null)} aria-label="Dismiss">
            ×
          </button>
        </div>
      )}

      {loading && visible.length === 0 ? (
        <div className="loading">
          <span>…</span>
        </div>
      ) : (
        <div className="cards-grid">
          {visible.map((s) => {
            const info = commentatorById[s.id];
            const active = commentatorIsActive(info);
            const live = !!info?.connected;
            const panelClass = commentatorPanelClass(info);
            const numClass = commentatorNumClass(info);
            const isBusy = !!busy[s.id];
            const intercomCount = info?.intercom?.filter((slot) => slot.enabled).length ?? 0;

            return (
              <div
                key={s.id}
                className={`card-panel ${s.status} ${panelClass}${active ? " commentator-active" : ""}${live ? " commentator-live" : ""}`}
              >
                <div className="card-stage">
                  <div className="card-thumb commentator-thumb">
                    <div className={`commentator-thumb-inner commentator-thumb-inner--${panelClass}`}>
                      <span className="commentator-thumb-icon" aria-hidden>
                        🎧
                      </span>
                      <span className="commentator-thumb-status">{commentatorThumbLabel(info)}</span>
                      {active && intercomCount > 0 && (
                        <span className="commentator-thumb-meta">{intercomCount} intercom</span>
                      )}
                    </div>
                  </div>
                </div>

                <div className="card-footer">
                  <div className="card-top">
                    <div className="card-identity">
                      <span className={`card-channel-num ${numClass}`}>{s.id}</span>
                      <div className="card-identity-text">
                        <span className="card-name" title={info?.display_name?.trim() || s.name || `Channel ${s.id}`}>
                          {info?.display_name?.trim() || s.name || `Channel ${s.id}`}
                        </span>
                        <div className="card-meta">
                          <span className="card-meta-item">{commentatorCardMeta(info)}</span>
                        </div>
                      </div>
                    </div>
                    <div className="card-actions">
                      {!info?.session_active ? (
                        <button
                          type="button"
                          className="tc-start-btn"
                          disabled={isBusy || !info?.enabled}
                          onClick={() =>
                            void run(s.id, async () => {
                              await createCommentatorSession(s.id);
                            })
                          }
                        >
                          {isBusy ? "…" : "INVITE"}
                        </button>
                      ) : (
                        <button
                          type="button"
                          className="tc-stop-btn"
                          disabled={isBusy}
                          onClick={() =>
                            void run(s.id, async () => {
                              await revokeCommentatorSession(s.id);
                            })
                          }
                        >
                          {isBusy ? "…" : "REVOKE"}
                        </button>
                      )}
                      <button
                        type="button"
                        className="badge settings-btn"
                        onClick={() => setSettingsId(s.id)}
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

      <CommentatorSettingsModal
        open={settingsId != null}
        channelId={settingsId}
        onClose={() => setSettingsId(null)}
        onSaved={() => {}}
      />
    </section>
  );
}
