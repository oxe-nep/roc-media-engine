"use client";

import { useEffect, useState } from "react";
import { mediaBase } from "@/lib/mediaBase";

interface ThumbnailProps {
  id: number;
  active: boolean;
  /** DeckLink has no input — keep last frame dimmed with a No signal badge. */
  lostSignal?: boolean;
  /** Default encode thumbs at /thumb/{id}; decode uses /hls/playout/{id}/thumb.jpg */
  path?: string;
}

export default function Thumbnail({ id, active, lostSignal = false, path }: ThumbnailProps) {
  const basePath = path ?? `/thumb/${id}`;
  const [src, setSrc] = useState(`${mediaBase()}${basePath}?t=${Date.now()}`);
  const [hasError, setHasError] = useState(!active);

  useEffect(() => {
    if (!active) {
      setHasError(true);
      return;
    }
    setSrc(`${mediaBase()}${basePath}?t=${Date.now()}`);
    const interval = setInterval(() => {
      setSrc(`${mediaBase()}${basePath}?t=${Date.now()}`);
    }, 1000);
    return () => clearInterval(interval);
  }, [id, active, basePath]);

  if (!active) {
    return <span className="no-signal">No signal</span>;
  }

  return (
    <>
      {hasError && !lostSignal && <span className="no-signal">No signal</span>}
      <img
        className={hasError && !lostSignal ? "thumb-hidden" : lostSignal ? "thumb-lost" : undefined}
        src={src}
        alt={`Channel ${id}`}
        onLoad={() => setHasError(false)}
        onError={() => setHasError(true)}
      />
      {lostSignal && !hasError && (
        <span className="thumb-lost-badge" aria-label="No signal">
          No signal
        </span>
      )}
    </>
  );
}
