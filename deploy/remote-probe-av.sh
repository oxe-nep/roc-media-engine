#!/usr/bin/env bash
set -euo pipefail
# Capture after 1.5s so we should start near/after an IDR (gop=50 @ 50fps).
timeout 6 ffmpeg -y -v error -f mpegts \
  -i 'udp://239.255.28.4:21004?localaddr=127.0.0.1&reuse=1&timeout=3000000' \
  -ss 1.5 -t 2 -c copy /tmp/ch4_ok.ts 2>/tmp/ch4_ok.err || true
echo "PPS_COUNT=$(grep -c non-existing /tmp/ch4_ok.err || true)"
ffprobe -v error -show_entries stream=index,codec_name,codec_type,profile -of csv=p=0 /tmp/ch4_ok.ts || true
# Switch HEVC briefly and check audio+video in TS
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/encode-preset' \
  -H 'Content-Type: application/json' -d '{"preset":"h_265_10mbit"}' >/tmp/hevc_set.json
sleep 4
timeout 5 ffmpeg -y -v error -f mpegts \
  -i 'udp://239.255.28.4:21004?localaddr=127.0.0.1&reuse=1&timeout=3000000' \
  -ss 1.0 -t 2 -c copy /tmp/ch4_hevc.ts 2>/tmp/ch4_hevc.err || true
echo "HEVC_STREAMS:"
ffprobe -v error -show_entries stream=index,codec_name,codec_type -of csv=p=0 /tmp/ch4_hevc.ts || true
# Back to proxy + SRT for player test
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/encode-preset' \
  -H 'Content-Type: application/json' -d '{"preset":"proxy"}' >/dev/null
sleep 3
SRT_URL='srt://10.199.23.251:8890?latency=1000000&mode=caller&pkt_size=1316&streamid=publish:live/main:publisher:3fQjvsAHKXE7GAyZ'
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/srt/start' \
  -H 'Content-Type: application/json' -d "{\"url\":\"$SRT_URL\"}" >/dev/null
sleep 2
curl -s http://127.0.0.1:8090/api/channels/4
echo
head -12 /opt/application/roc-recording/backend/hls/4/preview.m3u8
