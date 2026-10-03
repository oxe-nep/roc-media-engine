#!/bin/bash
set -euo pipefail
# shellcheck disable=SC1090
source "$HOME/.cargo/env"
cd /opt/applications/roc-media-engine
cargo build --release -p roc-engine --features gst 2>&1 | tail -8
sudo systemctl restart roc-media-engine
sleep 4
for id in 1 2 3 4 5 6 7 8; do
  curl -sf -m 45 -X POST "http://127.0.0.1:8090/api/channels/${id}/start" >/dev/null 2>&1 || true
done
sleep 10
for id in 1 2 3 4 5 6 7 8; do
  port=$((9100 + id))
  curl -sf -m 10 -X POST "http://127.0.0.1:8090/api/channels/${id}/srt/stop" >/dev/null 2>&1 || true
  sleep 1
  curl -sf -m 30 -X POST -H 'Content-Type: application/json' \
    -d "{\"url\":\"srt://0.0.0.0:${port}?mode=listener&latency=120\"}" \
    "http://127.0.0.1:8090/api/channels/${id}/srt/start" >/dev/null 2>&1 || true
done
sleep 12
echo "=== valve open logs ==="
journalctl -u roc-media-engine --since '60 sec ago' --no-pager \
  | grep 'SRT valve open' | tail -10
echo "=== srt capture ==="
timeout 10 ffmpeg -y -hide_banner -loglevel fatal \
  -i 'srt://127.0.0.1:9101?mode=caller&latency=200' -t 4 -c copy /tmp/srt_a5.ts || true
ffprobe -v error -show_entries stream=index,codec_type,codec_name -of csv=p=0 /tmp/srt_a5.ts || true
echo "=== cpu ==="
uptime
ps -eo pcpu,comm --sort=-pcpu | head -4
