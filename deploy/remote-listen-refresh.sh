#!/usr/bin/env bash
set -euo pipefail
cd /opt/applications/roc-media-engine
git pull
source "$HOME/.cargo/env"
cargo test -p roc-pipelines stereo_pair -- --nocapture
cargo build --release -p roc-engine --features gst
sudo systemctl stop roc-recording || true
for i in 1 2 3 4 5 6 7 8; do
  curl -s -o /dev/null -X POST "http://127.0.0.1:8090/api/channels/$i/stop" || true
done
sleep 1
sudo systemctl restart roc-media-engine
sleep 2
sudo systemctl start roc-recording
sleep 16
echo "=== hls/1 ==="
ls /opt/application/roc-recording/backend/hls/1/ | sort
echo "=== http ==="
for f in preview.m3u8 listen_0.m3u8 listen_1.m3u8 listen_2.m3u8 listen_3.m3u8; do
  code=$(curl -s -o /dev/null -w "%{http_code}" "http://127.0.0.1:8080/hls/1/$f")
  echo "$f=$code"
done
curl -s http://127.0.0.1:8090/health; echo
echo "=== errors ==="
sudo journalctl -u roc-media-engine --since "2 min ago" --no-pager | grep -iE "error|audiosrc|listen" | tail -25 || true
