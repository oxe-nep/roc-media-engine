#!/usr/bin/env bash
set -euo pipefail
BASE=http://127.0.0.1:8080
CH=4

curl -sS -X PUT "$BASE/api/streams/$CH/encode-preset" \
  -H 'Content-Type: application/json' -d '{"preset":"hq"}' >/dev/null
curl -sS -X PUT "$BASE/api/streams/$CH/record-preset" \
  -H 'Content-Type: application/json' -d '{"preset":"prores_proxy"}' | tee /tmp/rme-prores-preset.json
echo
curl -sS -X POST "$BASE/api/streams/$CH/start" >/dev/null || true
sleep 4
curl -sS "$BASE/api/channels" -o /tmp/rme-chs.json
python3 - <<'PY'
import json
c=next(x for x in json.load(open("/tmp/rme-chs.json"))["channels"] if x["id"]==4)
print("status", c["status"], "proxy", c.get("encode_preset"), "rec", c.get("record_preset"), "err", c.get("last_error"))
PY

curl -sS -X PUT "$BASE/api/recordings/$CH/name" \
  -H 'Content-Type: application/json' -d '{"name":"prores_smoke"}' >/dev/null
curl -sS -X PUT "$BASE/api/recordings/$CH/category" \
  -H 'Content-Type: application/json' -d '{"category":"_unsorted"}' >/dev/null
curl -sS -X POST "$BASE/api/recordings/$CH/start" | tee /tmp/rme-prores-rec.json
echo
sleep 15
curl -sS -X POST "$BASE/api/recordings/$CH/stop" | tee /tmp/rme-prores-stop.json
echo

FILE=$(ls -t /mnt/nep-storage/SHL/files/roc-recording/_unsorted/prores_smoke_ch4_*.mov 2>/dev/null | head -1 || true)
echo "FILE=$FILE"
if [ -n "$FILE" ]; then
  ls -la "$FILE"
  ffprobe -hide_banner "$FILE" 2>&1 | head -40
  if command -v mediainfo >/dev/null 2>&1; then
    mediainfo "$FILE" | head -50
  fi
else
  echo "no .mov"
  journalctl -u roc-media-engine --since "3 min ago" --no-pager | grep -iE "mezz|prores|error|WARN|assert" | tail -40
fi

curl -sS -X POST "$BASE/api/streams/$CH/stop" >/dev/null || true
curl -sS -X PUT "$BASE/api/streams/$CH/record-preset" \
  -H 'Content-Type: application/json' -d '{"preset":"dnxhd_hq"}' >/dev/null || true
