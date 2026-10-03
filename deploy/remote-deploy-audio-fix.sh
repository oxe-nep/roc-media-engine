#!/bin/bash
set -euo pipefail
# shellcheck disable=SC1090
source "$HOME/.cargo/env"
cd /opt/applications/roc-media-engine

cargo test -p roc-pipelines four_aac -- --nocapture
cargo build --release -p roc-engine --features gst
sudo systemctl restart roc-media-engine
sleep 4
systemctl is-active roc-media-engine

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
sleep 15

echo "=== load / cpu ==="
uptime
ps -eo pid,pcpu,pmem,comm --sort=-pcpu | head -5

echo "=== streams ch1 ==="
timeout 12 ffprobe -v error -show_entries stream=index,codec_type,codec_name \
  -of csv=p=0 "srt://127.0.0.1:9101?mode=caller&latency=200" 2>&1 | head -20

echo "=== valve log ==="
journalctl -u roc-media-engine --since '60 sec ago' --no-pager \
  | grep -iE 'audio_pairs|SRT valve open|ERROR|failed' | tail -20
