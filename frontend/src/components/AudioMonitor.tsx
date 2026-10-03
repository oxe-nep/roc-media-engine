"use client";

import { useEffect, useRef } from "react";
import type Hls from "hls.js";
import { mediaURL } from "@/lib/mediaBase";
import { attachHls, stopMedia } from "@/lib/hlsPlayer";

interface AudioMonitorProps {
  id: number;
  active: boolean;
  listening: boolean;
  /** Default /hls/{id}/audio.m3u8; decode uses /hls/playout/{id}/audio.m3u8 */
  playlistPath?: string;
}

export default function AudioMonitor({ id, active, listening, playlistPath }: AudioMonitorProps) {
  const audioRef = useRef<HTMLAudioElement>(null);
  const hlsRef = useRef<Hls | null>(null);

  useEffect(() => {
    const audio = audioRef.current;
    if (!audio) return;

    const stop = () => {
      stopMedia(audio, hlsRef.current);
      hlsRef.current = null;
    };

    if (!listening || !active) {
      stop();
      return;
    }

    const src = mediaURL(playlistPath ?? `/hls/${id}/audio.m3u8`);
    hlsRef.current = attachHls(audio, src, () => {});

    return stop;
  }, [id, active, listening, playlistPath]);

  return <audio ref={audioRef} className="audio-monitor" preload="none" />;
}
