import type Hls from "hls.js";

/**
 * Keep Encode-grid HLS tiles on the same wall-clock picture.
 *
 * Each channel has its own live playlist, so hls.js alone drifts. We sample
 * latency across visible previews and nudge playback (rate / small seeks) so
 * every tile sits near the same distance behind its live edge — matching the
 * intent of FFmpeg `hls_flags=…+program_date_time` (PDT not available on
 * GStreamer 1.28 hlssink2).
 */

type Tile = {
  el: HTMLVideoElement;
  hls: Hls;
};

const tiles = new Set<Tile>();
let timer: number | null = null;

function latencySec(tile: Tile): number | null {
  let sec = tile.hls.latency;
  if (!Number.isFinite(sec) || sec <= 0) {
    const b = tile.el.buffered;
    if (b.length === 0) return null;
    sec = b.end(b.length - 1) - tile.el.currentTime;
  }
  return Number.isFinite(sec) && sec > 0 ? sec : null;
}

function liveEdge(el: HTMLVideoElement): number | null {
  const b = el.buffered;
  if (b.length === 0) return null;
  return b.end(b.length - 1);
}

function seekToLatency(el: HTMLVideoElement, targetLat: number) {
  const edge = liveEdge(el);
  if (edge == null) return;
  const b = el.buffered;
  const floor = b.start(b.length - 1);
  const next = Math.min(edge - 0.05, Math.max(floor, edge - targetLat));
  if (Math.abs(el.currentTime - next) > 0.04) {
    el.currentTime = next;
  }
}

function tick() {
  const samples: { tile: Tile; lat: number }[] = [];
  for (const tile of tiles) {
    if (tile.el.paused || tile.el.readyState < 2) continue;
    const lat = latencySec(tile);
    if (lat == null) continue;
    samples.push({ tile, lat });
  }

  if (samples.length < 2) {
    for (const { tile } of samples) tile.el.playbackRate = 1;
    return;
  }

  const sorted = samples.map((s) => s.lat).sort((a, b) => a - b);
  // Bias toward the slower tile so ahead channels hold back (genlock MV).
  const target = sorted[Math.floor((sorted.length - 1) * 0.75)];

  for (const { tile, lat } of samples) {
    const delta = lat - target; // +behind live (older pic), −ahead (newer pic)
    if (delta < -0.28 || delta > 0.28) {
      seekToLatency(tile.el, target);
      tile.el.playbackRate = 1;
    } else if (delta < -0.06) {
      tile.el.playbackRate = 0.9; // ahead → slow down, grow latency
    } else if (delta > 0.06) {
      tile.el.playbackRate = 1.12; // behind → catch up
    } else {
      tile.el.playbackRate = 1;
    }
  }
}

function ensureTimer() {
  if (timer != null || typeof window === "undefined") return;
  timer = window.setInterval(tick, 400);
}

function stopTimerIfEmpty() {
  if (tiles.size > 0 || timer == null) return;
  window.clearInterval(timer);
  timer = null;
}

/** Register a visible preview video. Returns unregister. */
export function registerPreviewTile(el: HTMLVideoElement, hls: Hls): () => void {
  const tile: Tile = { el, hls };
  tiles.add(tile);
  ensureTimer();
  return () => {
    el.playbackRate = 1;
    tiles.delete(tile);
    stopTimerIfEmpty();
  };
}
