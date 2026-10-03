#!/bin/bash
# Minimal repro: does mpegtsmux keep 4 AAC when linked like the engine?
set -euo pipefail
timeout 8 gst-launch-1.0 -e \
  videotestsrc is-live=true num-buffers=100 ! video/x-raw,width=320,height=180,framerate=25/1 ! \
  x264enc tune=zerolatency ! h264parse ! mpegtsmux name=m alignment=7 ! \
  filesink location=/tmp/test4aac.ts \
  audiotestsrc is-live=true wave=ticks num-buffers=100 ! audio/x-raw,rate=48000,channels=2 ! voaacenc ! aacparse ! m. \
  audiotestsrc is-live=true wave=saw num-buffers=100 ! audio/x-raw,rate=48000,channels=2 ! voaacenc ! aacparse ! m. \
  audiotestsrc is-live=true wave=square num-buffers=100 ! audio/x-raw,rate=48000,channels=2 ! voaacenc ! aacparse ! m. \
  audiotestsrc is-live=true wave=silence num-buffers=100 ! audio/x-raw,rate=48000,channels=2 ! voaacenc ! aacparse ! m. \
  2>/tmp/test4aac.err || true
echo "=== ffprobe ==="
ffprobe -v error -show_entries stream=index,codec_type,codec_name -of csv=p=0 /tmp/test4aac.ts || true
echo "=== err ==="
tail -20 /tmp/test4aac.err || true
