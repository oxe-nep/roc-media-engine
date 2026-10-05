"use client";

import { Suspense, useMemo } from "react";
import { useParams, useSearchParams } from "next/navigation";
import WebRtcPreviewPlayer from "@/components/WebRtcPreviewPlayer";
import { DashboardProvider, useDashboard } from "@/hooks/useDashboard";

function PreviewWindowInner() {
  const params = useParams();
  const search = useSearchParams();
  const channelId = Number(params.id);
  const pair = Number(search.get("pair") ?? "0");
  const { streams, recordings } = useDashboard();

  const name = useMemo(() => {
    if (!Number.isFinite(channelId)) return "Preview";
    return (
      recordings[channelId]?.name ||
      streams.find((s) => s.id === channelId)?.name ||
      `ch${channelId}`
    );
  }, [channelId, recordings, streams]);

  if (!Number.isFinite(channelId) || channelId < 1) {
    return <div className="preview-window-error">Invalid channel</div>;
  }

  return (
    <div className="preview-window-root">
      <WebRtcPreviewPlayer
        channelId={channelId}
        channelName={name}
        initialPair={Number.isFinite(pair) ? Math.min(3, Math.max(0, pair)) : 0}
        variant="window"
      />
    </div>
  );
}

export default function PreviewWindowPage() {
  return (
    <DashboardProvider>
      <Suspense fallback={<div className="preview-window-error">Loading preview…</div>}>
        <PreviewWindowInner />
      </Suspense>
    </DashboardProvider>
  );
}
