import Hls from "hls.js";

/** Chromium stalls tab reload while hls.js workers are alive. */
const live = new Set<Hls>();

export const HLS_LIVE_CONFIG = {
  enableWorker: false,
  lowLatencyMode: false,
  liveSyncDurationCount: 3,
  liveMaxLatencyDurationCount: 10,
  maxLiveSyncPlaybackRate: 1,
  maxBufferLength: 8,
  maxMaxBufferLength: 16,
  startFragPrefetch: true,
} as const;

export function attachHls(
  el: HTMLMediaElement,
  src: string,
  onFatal: () => void,
): Hls | null {
  if (Hls.isSupported()) {
    const hls = new Hls({
      ...HLS_LIVE_CONFIG,
      xhrSetup(xhr) {
        xhr.setRequestHeader("Cache-Control", "no-cache");
      },
    });
    live.add(hls);
    hls.loadSource(src);
    hls.attachMedia(el);
    hls.on(Hls.Events.MANIFEST_PARSED, () => {
      void el.play().catch(() => {});
    });
    hls.on(Hls.Events.ERROR, (_ev, data) => {
      if (!data.fatal) return;
      live.delete(hls);
      hls.destroy();
      onFatal();
    });
    return hls;
  }
  if (el.canPlayType("application/vnd.apple.mpegurl")) {
    el.src = src;
    void el.play().catch(() => {});
  }
  return null;
}

export function stopMedia(el: HTMLMediaElement | null, hls: Hls | null) {
  if (hls) {
    live.delete(hls);
    try {
      hls.destroy();
    } catch {
      /* ignore */
    }
  }
  if (!el) return;
  el.pause();
  el.removeAttribute("src");
  el.srcObject = null;
  try {
    el.load();
  } catch {
    /* ignore */
  }
}

if (typeof window !== "undefined") {
  window.addEventListener("pagehide", () => {
    for (const h of [...live]) {
      try {
        h.destroy();
      } catch {
        /* ignore */
      }
    }
    live.clear();
  });
}
