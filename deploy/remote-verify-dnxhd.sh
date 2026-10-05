#!/usr/bin/env bash
set -euo pipefail
export PATH=/home/oxe/.cargo/bin:/usr/bin:/bin
BASE=http://127.0.0.1:8080
CH=4

curl -sS -X PUT "$BASE/api/streams/$CH/encode-preset" \
  -H 'Content-Type: application/json' \
  -d '{"preset":"dnxhd_185"}' | tee /tmp/rme-preset.json
echo
curl -sS -X POST "$BASE/api/streams/$CH/start" >/dev/null || true
sleep 4
curl -sS "$BASE/api/channels" -o /tmp/rme-chs.json
python3 - <<'PY'
import json
c=next(x for x in json.load(open("/tmp/rme-chs.json"))["channels"] if x["id"]==4)
print("status", c["status"], "preset", c["encode_preset"], "err", c.get("last_error"))
PY

curl -sS -X PUT "$BASE/api/recordings/$CH/name" \
  -H 'Content-Type: application/json' -d '{"name":"dnxhd_ntp_test"}' >/dev/null
curl -sS -X PUT "$BASE/api/recordings/$CH/category" \
  -H 'Content-Type: application/json' -d '{"category":"_unsorted"}' >/dev/null
curl -sS -X POST "$BASE/api/recordings/$CH/start" | tee /tmp/rme-rec.json
echo
sleep 10
curl -sS -X POST "$BASE/api/recordings/$CH/stop" | tee /tmp/rme-recstop.json
echo

ls -lt /opt/applications/roc-media-engine/recordings/_unsorted/ | head -10
FILE=$(ls -t /opt/applications/roc-media-engine/recordings/_unsorted/dnxhd_ntp_test_ch4_*.mxf 2>/dev/null | head -1 || true)
echo "FILE=$FILE"
if [ -n "$FILE" ]; then
  ls -la "$FILE"
  ffprobe -hide_banner "$FILE" 2>&1 | head -40
else
  echo "no .mxf produced"
  journalctl -u roc-media-engine --since "3 min ago" --no-pager | grep -iE "mezz|dnx|timecode|record|error|WARN" | tail -50
fi

curl -sS -X POST "$BASE/api/streams/$CH/stop" >/dev/null || true
curl -sS -X PUT "$BASE/api/streams/$CH/encode-preset" \
  -H 'Content-Type: application/json' -d '{"preset":"hq"}' >/dev/null || true
