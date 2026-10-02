#!/usr/bin/env bash
set -euo pipefail
F=/opt/applications/roc-media-engine/recordings/_unsorted/Channel_4_ch4_epoch20728_204417.mp4
echo "=== REC probe ==="
ls -la "$F"
ffprobe -v error -show_entries stream=codec_name,codec_type,profile,width,height -of csv=p=0 "$F" || true
echo "=== REC decode ==="
ffmpeg -y -v error -i "$F" -frames:v 10 -f null - 2>&1 | tail -30 || true
ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 "$F" 2>&1 || true
echo "=== switch to hq H.264 and retest UDP decode ==="
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/encode-preset' \
  -H 'Content-Type: application/json' -d '{"preset":"hq"}' >/dev/null
sleep 5
timeout 5 ffmpeg -y -v warning -f mpegts \
  -i 'udp://239.255.28.4:21004?localaddr=127.0.0.1&reuse=1&timeout=3000000' \
  -t 3 -c copy /tmp/ch4_h264.ts 2>/tmp/ch4_h264.err || true
ffprobe -v error -show_entries stream=codec_name,codec_type,profile -of csv=p=0 /tmp/ch4_h264.ts || true
echo "PPS_ERRS=$(grep -c 'non-existing PPS\|PPS id' /tmp/ch4_h264.err || true)"
echo "=== H264 decode frames ==="
ffmpeg -y -v error -i /tmp/ch4_h264.ts -an -frames:v 25 -f null - 2>&1 | tee /tmp/ch4_h264_dec.err | tail -20
grep -c 'non-existing PPS\|PPS id\|error' /tmp/ch4_h264_dec.err || true
