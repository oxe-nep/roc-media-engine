#!/usr/bin/env bash
# Probe: what actually breaks mxfmux on a live encode-once-style tee.
set -eu
rm -f /tmp/dnx-live*.mxf

echo "=== live tee hot-attach style (videotestsrc) ==="
gst-launch-1.0 -e \
  videotestsrc is-live=true ! videoconvert ! \
  video/x-raw,format=NV12,width=1920,height=1080,framerate=50/1 ! \
  tee name=t \
  t. ! queue ! videoconvert ! video/x-raw,format=NV12 ! fakesink sync=false \
  t. ! queue max-size-buffers=8 max-size-time=1000000000 ! \
    videoconvert ! video/x-raw,format=Y42B ! \
    timecodestamper source=rtc set=always ! \
    avenc_dnxhd bitrate=185000000 ! identity single-segment=true ! \
    mxfmux ! filesink location=/tmp/dnx-live1.mxf sync=false async=false &
PID=$!
sleep 3
kill -INT $PID 2>/dev/null || true
wait $PID 2>/dev/null || true
ls -la /tmp/dnx-live1.mxf 2>&1 || true

echo "=== sleep then add branch via gst-dynamic is hard; try without framerate on Y42B ==="
gst-launch-1.0 -e videotestsrc num-buffers=40 is-live=true ! videoconvert ! \
  video/x-raw,format=Y42B,width=1920,height=1080,framerate=50/1 ! \
  avenc_dnxhd bitrate=185000000 ! identity single-segment=true ! \
  mxfmux ! filesink location=/tmp/dnx-live2.mxf
ls -la /tmp/dnx-live2.mxf

echo "=== mix matrix via parse_launch like engine ==="
MATRIX='<<(1.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0),(0.0,1.0,0.0,0.0,0.0,0.0,0.0,0.0)>>'
gst-launch-1.0 -e audiotestsrc num-buffers=20 ! \
  audio/x-raw,channels=8,rate=48000,layout=interleaved ! \
  audioconvert mix-matrix="$MATRIX" ! \
  audio/x-raw,channels=2 ! fakesink && echo MIX_OK
