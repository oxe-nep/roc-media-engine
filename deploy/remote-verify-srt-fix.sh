#!/usr/bin/env bash
set -uo pipefail
for i in 1 2 3 4 5 6 7 8; do
  curl -s -X POST "http://127.0.0.1:8090/api/channels/$i/start" >/dev/null || true
done
sleep 10
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/encode-preset' \
  -H 'Content-Type: application/json' -d '{"preset":"hq"}' >/dev/null
sleep 3

# Engine listener with FFmpeg-style µs in URL — engine must normalize to ms
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/srt/stop' >/dev/null || true
sleep 1
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/srt/start' \
  -H 'Content-Type: application/json' \
  -d '{"url":"srt://0.0.0.0:19773?mode=listener&latency=1000000"}'
echo
sleep 2
journalctl -u roc-media-engine --since '30 sec ago' --no-pager | grep -E 'normalized SRT|attached SRT|channel=4' | tail -10

rm -f /tmp/eng_srt2.ts
timeout 8 ffmpeg -y -v warning -i 'srt://127.0.0.1:19773?mode=caller&latency=1000000' \
  -t 4 -c copy /tmp/eng_srt2.ts >/tmp/eng_rx2.log 2>&1 || true
echo "=== engine SRT (normalized) ==="
ls -la /tmp/eng_srt2.ts 2>/dev/null || true
ffprobe -v error -show_entries stream=codec_name,codec_type,profile,width,height -of csv=p=0 /tmp/eng_srt2.ts 2>&1 || true
ffmpeg -y -v error -i /tmp/eng_srt2.ts -frames:v 25 -f null - 2>&1 | tee /tmp/eng_dec2.err | tail -8 || true
echo "ERRS=$(grep -ciE 'error|PPS|non-existing|invalid' /tmp/eng_dec2.err || true)"
tail -15 /tmp/eng_rx2.log

# Point back at MediaMTX for the user
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/srt/stop' >/dev/null || true
sleep 1
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/srt/start' \
  -H 'Content-Type: application/json' \
  -d '{"url":"srt://10.199.23.251:8890?latency=1000000&mode=caller&pkt_size=1316&streamid=publish:live/main:publisher:3fQjvsAHKXE7GAyZ"}'
echo
sleep 3
curl -s http://127.0.0.1:8090/api/channels/4
echo
journalctl -u roc-media-engine --since '20 sec ago' --no-pager | grep normalized | tail -5
