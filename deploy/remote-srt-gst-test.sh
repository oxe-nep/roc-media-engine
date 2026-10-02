#!/usr/bin/env bash
set -uo pipefail

echo "=== ports / gst srtsink check ==="
ss -ulnp | grep -E '19771|9104|8890' || true
ss -tlnp | grep -E '19771|9104|8890' || true

# 1) Dump MPEG-TS via UDP (already known) and via a one-shot gst filesink from same encode —
#    but easier: use engine UDP + also capture with gst-launch from udpsrc to validate.
timeout 4 ffmpeg -y -v warning -f mpegts \
  -i 'udp://239.255.28.4:21004?localaddr=127.0.0.1&reuse=1&timeout=2500000' \
  -t 2 -c copy /tmp/ch4_master.ts 2>/tmp/m.err || true
echo "MASTER:"
ffprobe -v error -show_entries stream=codec_name,codec_type,profile -of csv=p=0 /tmp/ch4_master.ts || true
ffmpeg -y -v error -i /tmp/ch4_master.ts -frames:v 15 -f null - 2>&1 | tail -5 || true

# 2) Pure GST SRT loopback (no engine): listen + talk
rm -f /tmp/gst_srt_out.ts
timeout 6 gst-launch-1.0 -e \
  srtsrc uri='srt://:19880?mode=listener&latency=1000000' wait-for-connection=false ! \
  filesink location=/tmp/gst_srt_out.ts >/tmp/gst_rx.log 2>&1 &
RX=$!
sleep 1
timeout 4 gst-launch-1.0 -e \
  videotestsrc is-live=true num-buffers=100 ! video/x-raw,width=320,height=180,framerate=25/1 ! \
  x264enc tune=zerolatency key-int-max=25 ! h264parse config-interval=-1 ! \
  mpegtsmux alignment=7 ! \
  srtsink uri='srt://127.0.0.1:19880?mode=caller&latency=1000000' wait-for-connection=false \
  >/tmp/gst_tx.log 2>&1 || true
sleep 1
kill $RX 2>/dev/null || true
wait $RX 2>/dev/null || true
echo "=== gst SRT loop ==="
ls -la /tmp/gst_srt_out.ts 2>/dev/null || true
ffprobe -v error -show_entries stream=codec_name,codec_type -of csv=p=0 /tmp/gst_srt_out.ts 2>&1 || true
echo "RX:"; tail -15 /tmp/gst_rx.log
echo "TX:"; tail -15 /tmp/gst_tx.log

# 3) Engine listener + gst srtsrc caller
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/srt/stop' >/dev/null || true
sleep 1
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/srt/start' \
  -H 'Content-Type: application/json' \
  -d '{"url":"srt://:19772?mode=listener&latency=1000000"}'
echo
sleep 2
ss -ulnp | grep 19772 || ss -tlnp | grep 19772 || echo 'no 19772 listen'
rm -f /tmp/eng_srt.ts
timeout 8 gst-launch-1.0 -e \
  srtsrc uri='srt://127.0.0.1:19772?mode=caller&latency=1000000' ! \
  filesink location=/tmp/eng_srt.ts >/tmp/eng_rx.log 2>&1 || true
echo "=== engine→gst SRT ==="
ls -la /tmp/eng_srt.ts 2>/dev/null || true
ffprobe -v error -show_entries stream=codec_name,codec_type,profile,width,height -of csv=p=0 /tmp/eng_srt.ts 2>&1 || true
ffmpeg -y -v error -i /tmp/eng_srt.ts -frames:v 20 -f null - 2>&1 | tee /tmp/eng_dec.err | tail -10 || true
echo "ERRS=$(grep -ciE 'error|PPS|non-existing|invalid' /tmp/eng_dec.err || true)"
tail -20 /tmp/eng_rx.log

# restore MediaMTX
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/srt/stop' >/dev/null || true
sleep 1
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/srt/start' \
  -H 'Content-Type: application/json' \
  -d '{"url":"srt://10.199.23.251:8890?latency=1000000&mode=caller&pkt_size=1316&streamid=publish:live/main:publisher:3fQjvsAHKXE7GAyZ"}'
echo
curl -s http://127.0.0.1:8090/api/channels/4
echo
