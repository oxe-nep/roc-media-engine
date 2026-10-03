#!/bin/bash
set -uo pipefail
echo "=== ping mtx ==="
ping -c 2 -W 1 10.199.23.251 || true
echo "=== ch2 status ==="
curl -sf http://127.0.0.1:8090/api/channels/2 || true
echo
echo "=== read variants ==="
for sid in \
  'read:live/main' \
  'read:live/main:publisher:3fQjvsAHKXE7GAyZ' \
  'play:live/main' \
  'live/main'
do
  echo "SID=$sid"
  timeout 4 ffmpeg -hide_banner -loglevel warning \
    -i "srt://10.199.23.251:8890?mode=caller&latency=1000&streamid=${sid}" \
    -t 1 -f null - 2>&1 | tail -6 || true
  echo
done
echo "=== gst minimal publish ==="
timeout 6 gst-launch-1.0 -e \
  videotestsrc is-live=true num-buffers=80 ! video/x-raw,width=320,height=180,framerate=25/1 ! \
  x264enc tune=zerolatency speed-preset=ultrafast ! h264parse ! mpegtsmux name=m alignment=7 ! \
  srtsink uri='srt://10.199.23.251:8890?mode=caller&latency=1000&streamid=publish:live/main:publisher:3fQjvsAHKXE7GAyZ' \
  wait-for-connection=false \
  audiotestsrc is-live=true num-buffers=80 ! audio/x-raw,rate=48000,channels=2 ! \
  voaacenc ! aacparse ! m. 2>&1 | tail -20 || true
sleep 1
echo "=== read after gst publish ==="
timeout 5 ffmpeg -hide_banner -loglevel info \
  -i 'srt://10.199.23.251:8890?mode=caller&latency=1000&streamid=read:live/main:publisher:3fQjvsAHKXE7GAyZ' \
  -t 1 -f null - 2>&1 | grep -E 'Stream #|Program|Input|error|Connection|401|404' | head -20 || true
