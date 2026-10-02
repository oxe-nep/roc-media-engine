#!/usr/bin/env bash
set -euo pipefail
for i in 1 2 3 4 5 6 7 8; do
  echo -n "start ch$i: "
  curl -s -X POST "http://127.0.0.1:8090/api/channels/$i/start" || true
  echo
done
sleep 12
echo "=== ch4 ==="
curl -s "http://127.0.0.1:8090/api/channels/4"
echo
echo "=== launch snippet ==="
journalctl -u roc-media-engine --since "2 min ago" --no-pager \
  | grep -E "capture pipeline channel=4" | tail -1 \
  | grep -oE "nvh26[45]enc[^!]+|videorate[^!]*|pv[0-9]+_|drop-only|repeat-sequence|zerolatency|bframes=0" \
  | head -20
echo "=== set proxy + SRT ==="
# Re-apply proxy preset (relaunch) then attach previous MediaMTX caller URL from Go if present.
curl -s -X POST "http://127.0.0.1:8090/api/channels/4/encode-preset" \
  -H 'Content-Type: application/json' \
  -d '{"preset":"proxy"}'
echo
sleep 3
SRT_URL='srt://10.199.23.251:8890?latency=1000000&mode=caller&pkt_size=1316&streamid=publish:live/main:publisher:3fQjvsAHKXE7GAyZ'
curl -s -X POST "http://127.0.0.1:8090/api/channels/4/srt/start" \
  -H 'Content-Type: application/json' \
  -d "{\"url\":\"$SRT_URL\"}"
echo
sleep 4
curl -s "http://127.0.0.1:8090/api/channels/4"
echo
echo "=== UDP TS probe ==="
timeout 4 ffmpeg -y -v warning -f mpegts \
  -i 'udp://239.255.28.4:21004?localaddr=127.0.0.1&reuse=1&timeout=2500000' \
  -t 2 -c copy /tmp/ch4_fix.ts || true
ffprobe -v error -show_entries stream=index,codec_name,codec_type,profile,width,height \
  -of csv=p=0 /tmp/ch4_fix.ts || true
echo "=== PPS join check (expect few/no non-existing PPS after IDR) ==="
timeout 4 ffmpeg -y -v error -f mpegts \
  -i 'udp://239.255.28.4:21004?localaddr=127.0.0.1&reuse=1&timeout=2500000' \
  -t 3 -c copy /tmp/ch4_join.ts 2>/tmp/ch4_join.err || true
grep -c "non-existing PPS" /tmp/ch4_join.err || true
head -5 /tmp/ch4_join.err || true
echo "=== HLS playlist ==="
sleep 2
head -20 /opt/application/roc-recording/backend/hls/4/preview.m3u8 || true
ls -lt /opt/application/roc-recording/backend/hls/4/ | head -12
