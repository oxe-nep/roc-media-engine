#!/usr/bin/env bash
set -euo pipefail
BASE=http://127.0.0.1:8080
CH=4

# Ensure mezz preset has 8ch PCM
curl -sS -X PUT "$BASE/api/encode/presets/dnxhd_hq" \
  -H 'Content-Type: application/json' \
  -d '{"label":"DNxHD HQ","video_codec":"avenc_dnxhd","video_bitrate":"185M","video_preset":"hq","video_gop":1,"audio_bitrate":"384k","audio_channels":8}' \
  | tee /tmp/rme-dnx-8ch-preset.json
echo

curl -sS -X PUT "$BASE/api/streams/$CH/encode-preset" \
  -H 'Content-Type: application/json' -d '{"preset":"hq"}' >/dev/null
curl -sS -X PUT "$BASE/api/streams/$CH/record-preset" \
  -H 'Content-Type: application/json' -d '{"preset":"dnxhd_hq"}' >/dev/null
curl -sS -X POST "$BASE/api/streams/$CH/start" >/dev/null || true
sleep 4

curl -sS -X PUT "$BASE/api/recordings/$CH/name" \
  -H 'Content-Type: application/json' -d '{"name":"dnxhd_8ch_test"}' >/dev/null
curl -sS -X PUT "$BASE/api/recordings/$CH/category" \
  -H 'Content-Type: application/json' -d '{"category":"_unsorted"}' >/dev/null
curl -sS -X POST "$BASE/api/recordings/$CH/start"; echo
sleep 8
curl -sS -X POST "$BASE/api/recordings/$CH/stop"; echo

FILE=$(ls -t /opt/applications/roc-media-engine/recordings/_unsorted/dnxhd_8ch_test_ch4_*.mxf 2>/dev/null | head -1 || true)
echo "FILE=$FILE"
if [ -n "$FILE" ]; then
  ls -la "$FILE"
  ffprobe -hide_banner "$FILE" 2>&1 | head -45
else
  journalctl -u roc-media-engine --since "2 min ago" --no-pager | grep -iE "mezz|pcm|audio_pads|error|WARN|assert" | tail -40
fi

curl -sS -X POST "$BASE/api/streams/$CH/stop" >/dev/null || true
