"use client";

import { useEffect, useState } from "react";
import { mediaBase } from "@/lib/mediaBase";

interface ThumbnailProps {
  id: number;
  active: boolean;
  /** Default encode thumbs at /thumb/{id}; decode uses /hls/playout/{id}/thumb.jpg */
  path?: string;
}

export default function Thumbnail({ id, active, path }: ThumbnailProps) {
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
    return <span className="no-signal" aria-hidden />;
  }

  return (
    <>
      {hasError && <span className="no-signal" aria-hidden />}
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
