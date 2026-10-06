#!/usr/bin/env bash
set -euo pipefail
BASE=http://127.0.0.1:8080
CH=4
curl -sS -X PUT "$BASE/api/streams/$CH/encode-preset" \
  -H 'Content-Type: application/json' -d '{"preset":"hq"}' >/dev/null
curl -sS -X PUT "$BASE/api/streams/$CH/record-preset" \
  -H 'Content-Type: application/json' -d '{"preset":"prores_proxy"}'
echo
curl -sS -X POST "$BASE/api/streams/$CH/start" >/dev/null || true
sleep 5
curl -sS -X PUT "$BASE/api/recordings/$CH/name" \
  -H 'Content-Type: application/json' -d '{"name":"prores_proxy_smoke"}' >/dev/null
curl -sS -X PUT "$BASE/api/recordings/$CH/category" \
  -H 'Content-Type: application/json' -d '{"category":"_unsorted"}' >/dev/null
curl -sS -X POST "$BASE/api/recordings/$CH/start"
echo
sleep 12
curl -sS -X POST "$BASE/api/recordings/$CH/stop"
echo
FILE=$(ls -t /mnt/nep-storage/SHL/files/roc-recording/_unsorted/prores_proxy_smoke_ch4_*.mov 2>/dev/null | head -1 || true)
echo "FILE=$FILE"
if [ -n "$FILE" ]; then
  ls -la "$FILE"
  ffprobe -hide_banner "$FILE" 2>&1 | head -40
fi
curl -sS -X POST "$BASE/api/streams/$CH/stop" >/dev/null || true
