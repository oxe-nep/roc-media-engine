#!/bin/bash
set -euo pipefail
source "$HOME/.cargo/env"
cd /opt/applications/roc-media-engine
cargo build --release -p roc-engine --features gst 2>&1 | tail -8
sudo systemctl restart roc-media-engine
sleep 4
for id in 1 2 3 4 5 6 7 8; do
  curl -sf -m 45 -X POST "http://127.0.0.1:8090/api/channels/${id}/start" >/dev/null 2>&1 || true
done
sleep 12
# ch1: local VLC listener
curl -sf -m 10 -X POST "http://127.0.0.1:8090/api/channels/1/srt/stop" >/dev/null 2>&1 || true
sleep 1
curl -sf -m 30 -X POST -H 'Content-Type: application/json' \
  -d '{"url":"srt://0.0.0.0:9101?mode=listener&latency=120"}' \
  "http://127.0.0.1:8090/api/channels/1/srt/start" >/dev/null
# ch2: MediaMTX publish (same path Go/UI uses)
curl -sf -m 10 -X POST "http://127.0.0.1:8090/api/channels/2/srt/stop" >/dev/null 2>&1 || true
sleep 1
curl -sf -m 30 -X POST -H 'Content-Type: application/json' \
  -d '{"url":"srt://10.199.23.251:8890?latency=1000000&mode=caller&pkt_size=1316&streamid=publish:live/main:publisher:3fQjvsAHKXE7GAyZ"}' \
  "http://127.0.0.1:8090/api/channels/2/srt/start" >/dev/null
sleep 8
echo "=== journal ==="
journalctl -u roc-media-engine --since '60 sec ago' --no-pager \
  | grep -E 'SRT valve open|waiting for full|stereo AAC|PMT gate' | tail -20
echo "=== VLC/listener 9101 ==="
timeout 5 ffmpeg -hide_banner -loglevel info \
  -i 'srt://127.0.0.1:9101?mode=caller&latency=200' -t 2 -f null - 2>&1 \
  | grep -E 'Stream #0:|Program|error|Invalid' | head -20
echo "=== MediaMTX read live/main ==="
timeout 6 ffmpeg -hide_banner -loglevel info \
  -i 'srt://10.199.23.251:8890?mode=caller&latency=1000&streamid=read:live/main:publisher:3fQjvsAHKXE7GAyZ' \
  -t 2 -f null - 2>&1 | grep -E 'Stream #0:|Program|error|Invalid|Connection|Input' | head -25
echo "=== cpu ==="
ps -eo pcpu,comm --sort=-pcpu | head -3
