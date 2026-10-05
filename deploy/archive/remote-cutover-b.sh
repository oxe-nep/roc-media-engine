#!/bin/bash
set -euo pipefail
source "$HOME/.cargo/env"
cd /opt/applications/roc-media-engine

# Sync sources expected in /tmp from scp
for f in main.rs api.rs orchestrator.rs; do
  if [[ -f "/tmp/roc-engine-src/$f" ]]; then
    cp -f "/tmp/roc-engine-src/$f" "crates/roc-engine/src/$f"
  fi
done
if [[ -d /tmp/roc-engine-src/ui ]]; then
  rm -rf crates/roc-engine/src/ui
  cp -a /tmp/roc-engine-src/ui crates/roc-engine/src/ui
fi
if [[ -f /tmp/roc-engine-Cargo.toml ]]; then
  cp -f /tmp/roc-engine-Cargo.toml crates/roc-engine/Cargo.toml
fi
if [[ -f /tmp/workspace-Cargo.toml ]]; then
  cp -f /tmp/workspace-Cargo.toml Cargo.toml
fi
if [[ -f /tmp/roc-config-lib.rs ]]; then
  cp -f /tmp/roc-config-lib.rs crates/roc-config/src/lib.rs
fi
if [[ -f /tmp/capture.rs ]]; then
  cp -f /tmp/capture.rs crates/roc-pipelines/src/gst/capture.rs
fi
if [[ -f /tmp/config.yaml ]]; then
  cp -f /tmp/config.yaml config.yaml
fi

export ROC_MEDIA_HLS_DIR="${ROC_MEDIA_HLS_DIR:-/opt/applications/roc-recording/backend/hls}"
export ROC_MEDIA_PUBLIC_HOST="${ROC_MEDIA_PUBLIC_HOST:-10.199.28.249}"

cargo build --release -p roc-engine --features gst 2>&1 | tail -20
sudo systemctl restart roc-media-engine
sleep 4

# Stop Go backend so it cannot overwrite presets / own ports.
if systemctl list-unit-files | grep -q '^roc-recording\.service'; then
  sudo systemctl disable --now roc-recording 2>/dev/null || sudo systemctl stop roc-recording || true
  echo "stopped roc-recording (Go)"
fi
# Common alternate unit names
for u in roc-recording-backend recording-backend; do
  if systemctl list-unit-files | grep -q "^${u}\\.service"; then
    sudo systemctl disable --now "$u" 2>/dev/null || true
  fi
done

for id in 1 2 3 4 5 6 7 8; do
  curl -sf -m 45 -X POST "http://127.0.0.1:8090/api/channels/${id}/start" >/dev/null 2>&1 || true
done
sleep 8

echo "=== health ==="
curl -sf http://127.0.0.1:8090/api/health | head -c 300; echo
echo "=== streams ==="
curl -sf http://127.0.0.1:8090/api/streams | python3 -c 'import sys,json; d=json.load(sys.stdin); print(len(d), [x.get("status") for x in d[:3]])'
echo "=== presets ==="
curl -sf http://127.0.0.1:8090/api/encode/presets | python3 -c 'import sys,json; d=json.load(sys.stdin); print([(p["id"], p.get("audio_channels")) for p in d])'
echo "=== srt start ch1 listener ==="
curl -sf -m 10 -X POST http://127.0.0.1:8090/api/srt/1/stop >/dev/null 2>&1 || true
curl -sf -m 30 -X PUT -H 'Content-Type: application/json' \
  -d '{"mode":"listener","port":9101,"latency_ms":120}' \
  http://127.0.0.1:8090/api/srt/1 >/dev/null
curl -sf -m 30 -X POST http://127.0.0.1:8090/api/srt/1/start | python3 -c 'import sys,json; print(json.load(sys.stdin))'
echo "=== ffmpeg 9101 ==="
timeout 8 ffmpeg -y -hide_banner -loglevel warning \
  -i 'srt://127.0.0.1:9101?mode=caller&latency=200' -t 3 -c copy /tmp/cutover_vlc.ts 2>&1 | tail -8 || true
ffprobe -v error -show_entries stream=codec_type,codec_name,channels -of csv=p=0 /tmp/cutover_vlc.ts 2>/dev/null || true
echo "=== hls ==="
ls -la "${ROC_MEDIA_HLS_DIR}/1/preview.m3u8" 2>/dev/null || ls -la /opt/applications/roc-recording/backend/hls/1/preview.m3u8 2>/dev/null || echo "no preview yet"
echo "=== go process ==="
pgrep -a 'roc-recording|recording-backend' || echo "no go backend process"
ps -eo pcpu,comm --sort=-pcpu | head -4
