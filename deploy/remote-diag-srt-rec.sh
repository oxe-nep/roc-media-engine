#!/usr/bin/env bash
set -euo pipefail
echo "=== ch4 status ==="
curl -s http://127.0.0.1:8090/api/channels/4
echo
echo "=== UDP capture 3s (master encode) ==="
timeout 5 ffmpeg -y -v warning -f mpegts \
  -i 'udp://239.255.28.4:21004?localaddr=127.0.0.1&reuse=1&timeout=3000000' \
  -t 3 -c copy /tmp/ch4_udp.ts 2>/tmp/ch4_udp.err || true
ls -la /tmp/ch4_udp.ts 2>/dev/null || true
ffprobe -v error -show_entries stream=index,codec_name,codec_type,profile,width,height,avg_frame_rate \
  -of json /tmp/ch4_udp.ts 2>&1 | head -80
echo "UDP_ERR:"; head -20 /tmp/ch4_udp.err || true
echo "=== decode smoke UDP ==="
timeout 8 ffmpeg -y -v error -i /tmp/ch4_udp.ts -frames:v 5 -f null - 2>&1 | tail -20
echo "=== SRT listener sniff on engine side? ==="
# Attach a temporary filesink by reading from UDP is enough for master;
# For SRT: ask MediaMTX API if any
curl -s http://10.199.23.251:9997/v3/paths/list 2>/dev/null | head -c 2000 || true
echo
curl -s http://127.0.0.1:9997/v3/paths/list 2>/dev/null | head -c 2000 || true
echo
echo "=== start short REC ==="
REC=/tmp/ch4_rec_test.mp4
rm -f "$REC"
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/record/start' \
  -H 'Content-Type: application/json' -d "{\"path\":\"$REC\"}"
echo
sleep 4
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/record/stop'
echo
sleep 1
ls -la "$REC" 2>/dev/null || true
ffprobe -v error -show_entries stream=index,codec_name,codec_type,profile,width,height \
  -of csv=p=0 "$REC" 2>&1 || true
timeout 8 ffmpeg -y -v error -i "$REC" -frames:v 5 -f null - 2>&1 | tail -15
echo "=== gst pipeline elements on ch4 (srt parse props) ==="
journalctl -u roc-media-engine --since '2 min ago' --no-pager \
  | grep -E 'attached SRT|REC|warn|error|video-only' | tail -20
