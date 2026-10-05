#!/usr/bin/env bash
set -euo pipefail
rm -f /tmp/dnx-late2.mxf /tmp/dnx-avmux.mxf /tmp/dnx-ts.mxf

echo "=== identity after enc with timestamp-offset ==="
set +e
gst-launch-1.0 -e \
  videotestsrc num-buffers=30 is-live=true timestamp-offset=3600000000000000 ! \
  videoconvert ! video/x-raw,format=Y42B,width=1920,height=1080,framerate=50/1 ! \
  avenc_dnxhd bitrate=185000000 ! identity single-segment=true ! \
  mxfmux ! filesink location=/tmp/dnx-late2.mxf
echo exit=$?
ls -la /tmp/dnx-late2.mxf 2>&1
set -e

echo "=== avmux_mxf ==="
set +e
gst-launch-1.0 -e \
  videotestsrc num-buffers=30 is-live=true ! \
  videoconvert ! video/x-raw,format=Y42B,width=1920,height=1080,framerate=50/1 ! \
  avenc_dnxhd bitrate=185000000 ! avmux_mxf ! filesink location=/tmp/dnx-avmux.mxf
echo exit=$?
ls -la /tmp/dnx-avmux.mxf 2>&1
ffprobe -hide_banner /tmp/dnx-avmux.mxf 2>&1 | head -20
set -e

echo "=== matroskamux fallback ==="
set +e
gst-launch-1.0 -e \
  videotestsrc num-buffers=30 is-live=true timestamp-offset=3600000000000000 ! \
  videoconvert ! video/x-raw,format=Y42B,width=1920,height=1080,framerate=50/1 ! \
  avenc_dnxhd bitrate=185000000 ! identity single-segment=true ! \
  matroskamux ! filesink location=/tmp/dnx-mkv.mkv
echo exit=$?
ls -la /tmp/dnx-mkv.mkv 2>&1
set -e

echo "=== mix-matrix ==="
gst-launch-1.0 -e \
  audiotestsrc num-buffers=10 ! audio/x-raw,channels=8,rate=48000,layout=interleaved ! \
  audioconvert 'mix-matrix=<<(1.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0),(0.0,1.0,0.0,0.0,0.0,0.0,0.0,0.0)>>' ! \
  audio/x-raw,format=S24LE,channels=2,rate=48000,layout=interleaved ! fakesink
echo mix-matrix-ok
