#!/usr/bin/env bash
set -euo pipefail
source ~/.cargo/env
cd /opt/applications/roc-media-engine
cargo build --release -p roc-engine --features gst 2>&1 | tail -8
sudo systemctl restart roc-media-engine
sleep 2
for i in 1 2 3 4 5 6 7 8; do
  curl -s -X POST "http://127.0.0.1:8090/api/channels/$i/start" >/dev/null || true
done
sleep 12
echo "health=$(curl -s http://127.0.0.1:8090/health)"
echo "ch4=$(curl -s -X POST http://127.0.0.1:8090/api/channels/4/start)"
sleep 2
curl -s http://127.0.0.1:8090/api/channels/4
echo
ls -la /opt/application/roc-recording/backend/hls/4/*.m3u8 2>/dev/null || echo "no m3u8 yet"
SEG=$(grep '\.ts$' /opt/application/roc-recording/backend/hls/4/listen_0.m3u8 2>/dev/null | head -1 || true)
echo "seg=$SEG"
if [ -n "${SEG:-}" ]; then
  ffprobe -hide_banner "/opt/application/roc-recording/backend/hls/4/$SEG" 2>&1 | head -20
fi
echo '=== errors ==='
journalctl -u roc-media-engine --since '1 min ago' --no-pager | grep -iE 'error|assertion|fail|parse capture' | tail -15 || true
