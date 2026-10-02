#!/usr/bin/env bash
set -euo pipefail
sudo systemctl restart roc-media-engine
sleep 3
# Go will re-sync presets; also restart so it re-adopts channels
sudo systemctl restart roc-recording
sleep 10
for i in 1 2 3 4 5 6 7 8; do
  curl -s -X POST "http://127.0.0.1:8090/api/channels/$i/start" >/dev/null || true
done
sleep 8
curl -s http://127.0.0.1:8090/api/channels/4
echo
sed -i 's/\r$//' /tmp/remote-srt-loopback.sh
bash /tmp/remote-srt-loopback.sh
