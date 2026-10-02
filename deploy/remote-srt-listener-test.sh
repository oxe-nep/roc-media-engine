#!/usr/bin/env bash
set -uo pipefail
echo "=== previous rx log ==="
cat /tmp/ch4_srt_rx.log 2>/dev/null | tail -40 || true
echo "=== journal srt ==="
journalctl -u roc-media-engine --since '3 min ago' --no-pager | grep -iE 'srt|error|warn|19770' | tail -30

# Stop any leftover SRT on ch4
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/srt/stop' >/dev/null || true
sleep 1

# Engine as LISTENER, ffmpeg as CALLER (more reliable for capture)
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/encode-preset' \
  -H 'Content-Type: application/json' -d '{"preset":"hq"}' >/dev/null
sleep 2
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/srt/start' \
  -H 'Content-Type: application/json' \
  -d '{"url":"srt://0.0.0.0:19771?mode=listener&latency=1000000"}'
echo
sleep 2
curl -s http://127.0.0.1:8090/api/channels/4
echo

rm -f /tmp/ch4_srt_loop.ts
timeout 8 ffmpeg -y -v info -i 'srt://127.0.0.1:19771?mode=caller&latency=1000000' \
  -t 4 -c copy /tmp/ch4_srt_loop.ts >/tmp/ch4_srt_rx2.log 2>&1 || true

echo "=== result ==="
ls -la /tmp/ch4_srt_loop.ts 2>/dev/null || true
ffprobe -v error -show_entries stream=codec_name,codec_type,profile,width,height -of csv=p=0 /tmp/ch4_srt_loop.ts 2>&1 || true
ffmpeg -y -v error -i /tmp/ch4_srt_loop.ts -frames:v 20 -f null - 2>&1 | tee /tmp/ch4_srt_dec.err | tail -15 || true
echo "ERRS=$(grep -ciE 'error|PPS|non-existing|invalid' /tmp/ch4_srt_dec.err || true)"
echo "=== rx2 log ==="
tail -40 /tmp/ch4_srt_rx2.log || true

# Restore MediaMTX caller URL for user
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/srt/stop' >/dev/null || true
sleep 1
SRT_URL='srt://10.199.23.251:8890?latency=1000000&mode=caller&pkt_size=1316&streamid=publish:live/main:publisher:3fQjvsAHKXE7GAyZ'
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/srt/start' \
  -H 'Content-Type: application/json' -d "{\"url\":\"$SRT_URL\"}"
echo
curl -s http://127.0.0.1:8090/api/channels/4
echo
