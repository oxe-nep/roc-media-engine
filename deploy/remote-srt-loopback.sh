#!/usr/bin/env bash
set -euo pipefail
# Local SRT loopback: engine → listener on :19770 → file, then ffprobe/decode.
LISTENER='srt://127.0.0.1:19770?mode=listener&latency=1000000'
CALLER='srt://127.0.0.1:19770?mode=caller&latency=1000000&pkt_size=1316'

# Ensure H.264 for player-like test
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/encode-preset' \
  -H 'Content-Type: application/json' -d '{"preset":"hq"}' >/dev/null
sleep 3

# Start receiver in background
rm -f /tmp/ch4_srt_loop.ts
timeout 12 ffmpeg -y -v warning -i "$LISTENER" -c copy /tmp/ch4_srt_loop.ts \
  >/tmp/ch4_srt_rx.log 2>&1 &
RX=$!
sleep 1

curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/srt/start' \
  -H 'Content-Type: application/json' \
  -d "{\"url\":\"$CALLER\"}"
echo
sleep 6
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/srt/stop' >/dev/null || true
wait $RX || true

echo "=== SRT loop file ==="
ls -la /tmp/ch4_srt_loop.ts || true
ffprobe -v error -show_entries stream=codec_name,codec_type,profile,width,height -of csv=p=0 /tmp/ch4_srt_loop.ts || true
echo "=== decode ==="
ffmpeg -y -v error -i /tmp/ch4_srt_loop.ts -frames:v 20 -f null - 2>&1 | tee /tmp/ch4_srt_dec.err | tail -15
echo "ERRS=$(grep -ciE 'error|PPS|non-existing|invalid' /tmp/ch4_srt_dec.err || true)"
echo "=== ch4 ==="
curl -s http://127.0.0.1:8090/api/channels/4
echo
cat /tmp/ch4_srt_rx.log | tail -20
