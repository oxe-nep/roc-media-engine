#!/bin/bash
set -uo pipefail
MTX='srt://10.199.23.251:8890'
PUB='publish:live/main:publisher:3fQjvsAHKXE7GAyZ'
READ='read:live/main:publisher:3fQjvsAHKXE7GAyZ'

echo "=== UDP 8890 ==="
nc -vz -u -w 2 10.199.23.251 8890 2>&1 || true

echo "=== engine ch2 -> MTX publish ==="
curl -sf -m 10 -X POST http://127.0.0.1:8090/api/channels/2/srt/stop >/dev/null || true
sleep 1
curl -sf -m 30 -X POST -H 'Content-Type: application/json' \
  -d "{\"url\":\"${MTX}?latency=1000000&mode=caller&pkt_size=1316&streamid=${PUB}\"}" \
  http://127.0.0.1:8090/api/channels/2/srt/start >/dev/null
sleep 5
journalctl -u roc-media-engine --since '30 sec ago' --no-pager \
  | grep -E 'channel=2.*(valve open|stereo|SRT|error|WARN)' | tail -15
curl -sf http://127.0.0.1:8090/api/channels/2 | python3 -m json.tool | grep -E 'srt|status|bitrate|error'

echo "=== ffmpeg read (while engine publishes) ==="
timeout 8 ffmpeg -hide_banner -loglevel info \
  -i "${MTX}?mode=caller&latency=1000&streamid=${READ}" \
  -t 3 -f null - 2>&1 | grep -E 'Stream #|Program|Input|error|Connection|PEER|reject|Invalid' | head -30

echo "=== ffmpeg publish smoke (stop engine ch2 first) ==="
curl -sf -m 10 -X POST http://127.0.0.1:8090/api/channels/2/srt/stop >/dev/null || true
sleep 2
timeout 10 ffmpeg -hide_banner -loglevel info -re \
  -f lavfi -i testsrc=size=640x360:rate=25 \
  -f lavfi -i sine=f=1000:r=48000 \
  -c:v libx264 -tune zerolatency -pix_fmt yuv420p -g 50 \
  -c:a aac -ar 48000 -ac 2 -b:a 128k \
  -f mpegts "${MTX}?mode=caller&latency=1000&pkt_size=1316&streamid=${PUB}" \
  2>&1 | tee /tmp/mtx_pub.log | tail -25 &
FPID=$!
sleep 4
echo "=== read during ffmpeg publish ==="
timeout 5 ffmpeg -hide_banner -loglevel info \
  -i "${MTX}?mode=caller&latency=1000&streamid=${READ}" \
  -t 2 -f null - 2>&1 | grep -E 'Stream #|Program|Input|error|Connection|PEER' | head -20
wait $FPID 2>/dev/null || true
echo "=== publish log tail ==="
grep -iE 'error|Connection|SRT|frame=|Output' /tmp/mtx_pub.log | tail -15
