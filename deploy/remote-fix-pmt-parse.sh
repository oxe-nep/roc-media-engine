#!/bin/bash
set -euo pipefail
# shellcheck disable=SC1090
source "$HOME/.cargo/env"
cd /opt/applications/roc-media-engine

echo "=== minimal 4aac mux ==="
bash /tmp/remote-test-4aac-mux.sh || true

echo "=== build engine ==="
cargo build --release -p roc-engine --features gst 2>&1 | tail -8
sudo systemctl restart roc-media-engine
sleep 4

curl -sf -m 60 -X POST http://127.0.0.1:8090/api/channels/1/start || true
sleep 8
curl -sf -m 10 -X POST http://127.0.0.1:8090/api/channels/1/srt/stop || true
sleep 1
curl -sf -m 30 -X POST -H 'Content-Type: application/json' \
  -d '{"url":"srt://0.0.0.0:9101?mode=listener&latency=120"}' \
  http://127.0.0.1:8090/api/channels/1/srt/start || true
sleep 20

echo "=== journal ==="
journalctl -u roc-media-engine --since '45 sec ago' --no-pager \
  | grep -E 'SRT valve|waiting for full|audio_pid|A/V input' | tail -25

echo "=== capture ==="
timeout 10 ffmpeg -y -hide_banner -loglevel fatal \
  -i 'srt://127.0.0.1:9101?mode=caller&latency=200' -t 4 -c copy /tmp/srt_a7.ts || true
ffprobe -v error -show_entries stream=index,codec_type,codec_name -of csv=p=0 /tmp/srt_a7.ts || true
ls -la /tmp/srt_a7.ts 2>/dev/null || true
