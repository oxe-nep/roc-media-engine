"use client";

import { useEffect, useState } from "react";
import { mediaBase } from "@/lib/mediaBase";

interface ThumbnailProps {
  id: number;
  active: boolean;
  /** DeckLink has no input — hide stale freeze-frame. */
  lostSignal?: boolean;
  /** Default encode thumbs at /thumb/{id}; decode uses /hls/playout/{id}/thumb.jpg */
  path?: string;
}

export default function Thumbnail({ id, active, lostSignal = false, path }: ThumbnailProps) {
  const basePath = path ?? `/thumb/${id}`;
  const [src, setSrc] = useState(`${mediaBase()}${basePath}?t=${Date.now()}`);
  const [hasError, setHasError] = useState(!active);

  useEffect(() => {
    if (!active || lostSignal) {
      setHasError(true);
      return;
    }
    setSrc(`${mediaBase()}${basePath}?t=${Date.now()}`);
    const interval = setInterval(() => {
      setSrc(`${mediaBase()}${basePath}?t=${Date.now()}`);
    }, 1000);
    return () => clearInterval(interval);
  }, [id, active, basePath, lostSignal]);

  if (!active || lostSignal) {
    return <span className="no-signal">No signal</span>;
  }

  return (
    <>
      {hasError && <span className="no-signal">No signal</span>}
      <img
        className={hasError ? "thumb-hidden" : undefined}
        src={src}
        alt={`Channel ${id}`}
        onLoad={() => setHasError(false)}
        onError={() => setHasError(true)}
      />
    </>
  );
}
