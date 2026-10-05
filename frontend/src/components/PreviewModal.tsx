"use client";

import { useEffect, useRef } from "react";
import WebRtcPreviewPlayer from "@/components/WebRtcPreviewPlayer";
import { useBodyScrollLock } from "@/hooks/useBodyScrollLock";

type Props = {
  channelId: number;
  channelName: string;
  open: boolean;
  initialPair?: number;
  onClose: () => void;
};

export default function PreviewModal({
  channelId,
  channelName,
  open,
  initialPair = 0,
  onClose,
}: Props) {
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  useBodyScrollLock(open);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCloseRef.current();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open]);

  if (!open) return null;

  return (
    <div
      className="modal-backdrop library-backdrop"
      onClick={() => onCloseRef.current()}
      role="presentation"
    >
      <div
        className="modal-panel preview-modal"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-label={`Preview ${channelName}`}
      >
        <WebRtcPreviewPlayer
          channelId={channelId}
          channelName={channelName}
          initialPair={initialPair}
          variant="modal"
          onClose={() => onCloseRef.current()}
        />
      </div>
    </div>
  );
}
