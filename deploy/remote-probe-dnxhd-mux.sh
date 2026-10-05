#!/usr/bin/env bash
set +e
echo '=== 1080p50 bitrates ==='
for br in 36000000 45000000 60000000 75000000 90000000 110000000 115000000 120000000 145000000 175000000 185000000 220000000; do
  echo -n "br=$br "
  if timeout 6 gst-launch-1.0 -q videotestsrc num-buffers=5 ! videoconvert ! \
    video/x-raw,format=Y42B,width=1920,height=1080,framerate=50/1 ! \
    avenc_dnxhd bitrate=$br ! fakesink >/tmp/dnx.out 2>/tmp/dnx.err; then
    echo OK
  else
    tail -2 /tmp/dnx.err | tr '\n' ' '; echo
  fi
done

echo '=== 1080p25 bitrates ==='
for br in 120000000 145000000 185000000 220000000; do
  echo -n "25p br=$br "
  if timeout 6 gst-launch-1.0 -q videotestsrc num-buffers=5 ! videoconvert ! \
    video/x-raw,format=Y42B,width=1920,height=1080,framerate=25/1 ! \
    avenc_dnxhd bitrate=$br ! fakesink >/tmp/dnx.out 2>/tmp/dnx.err; then
    echo OK
  else
    tail -2 /tmp/dnx.err | tr '\n' ' '; echo
  fi
done

echo '=== mux attempts @185M 50p ==='
for mux in matroskamux qtmux avmux_mov mp4mux mxfmux avmux_mxf; do
  echo -n "mux=$mux "
  rm -f /tmp/dnx-out.bin
  if timeout 8 gst-launch-1.0 -e videotestsrc num-buffers=20 ! videoconvert ! \
    video/x-raw,format=Y42B,width=1920,height=1080,framerate=50/1 ! \
    avenc_dnxhd bitrate=185000000 ! $mux ! filesink location=/tmp/dnx-out.bin \
    >/tmp/dnx.out 2>/tmp/dnx.err; then
    ls -la /tmp/dnx-out.bin | awk '{print "OK",$5}'
  else
    grep -iE 'could not link|ERROR|not-negotiated' /tmp/dnx.err | head -2 | tr '\n' ' '; echo
  fi
done

echo '=== caps after enc ==='
GST_DEBUG=2 timeout 5 gst-launch-1.0 videotestsrc num-buffers=2 ! videoconvert ! \
  video/x-raw,format=Y42B,width=1920,height=1080,framerate=50/1 ! \
  avenc_dnxhd bitrate=185000000 ! fakesink 2>&1 | grep -iE 'dnx|caps|error|negotiat' | head -20
