#!/usr/bin/env bash
set -uo pipefail
# Quick gst SRT loop with millisecond latency (libsrt units)
rm -f /tmp/gst_srt_out.ts
timeout 8 gst-launch-1.0 -e \
  srtsrc uri='srt://:19881?mode=listener&latency=120' wait-for-connection=true ! \
  filesink location=/tmp/gst_srt_out.ts >/tmp/gst_rx.log 2>&1 &
RX=$!
sleep 1
timeout 5 gst-launch-1.0 -e \
  videotestsrc is-live=true num-buffers=80 ! video/x-raw,width=320,height=180,framerate=25/1 ! \
  x264enc tune=zerolatency key-int-max=25 ! video/x-h264,stream-format=byte-stream ! \
  h264parse config-interval=-1 ! mpegtsmux alignment=7 ! \
  srtsink uri='srt://127.0.0.1:19881?mode=caller&latency=120' wait-for-connection=false \
  >/tmp/gst_tx.log 2>&1 || true
sleep 1
kill $RX 2>/dev/null || true
wait $RX 2>/dev/null || true
echo "GST loop size=$(wc -c </tmp/gst_srt_out.ts 2>/dev/null || echo 0)"
ffprobe -v error -show_entries stream=codec_name,codec_type -of csv=p=0 /tmp/gst_srt_out.ts 2>&1 || true
