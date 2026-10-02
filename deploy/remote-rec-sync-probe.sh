#!/usr/bin/env bash
set -uo pipefail
# Short REC and measure A/V start_time skew
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/encode-preset' \
  -H 'Content-Type: application/json' -d '{"preset":"hq"}' >/dev/null
sleep 2
PATH_OUT=/tmp/rec_sync_test.mp4
rm -f "$PATH_OUT"
curl -s -X POST "http://127.0.0.1:8090/api/channels/4/record/start?path=$PATH_OUT"
echo
sleep 6
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/record/stop'
echo
sleep 1
# Engine may rewrite path — find newest
F=$(ls -t /opt/applications/roc-media-engine/recordings/_unsorted/*.mp4 2>/dev/null | head -1)
echo "FILE=$F"
ls -la "$F" "$PATH_OUT" 2>/dev/null || true
for f in "$PATH_OUT" "$F"; do
  [ -f "$f" ] || continue
  echo "=== $f ==="
  ffprobe -v error -show_entries stream=index,codec_type,codec_name,start_time,duration,nb_frames \
    -of csv=p=0 "$f"
  # Compare first packet PTS
  ffprobe -v error -select_streams v:0 -show_entries packet=pts_time -of csv=p=0 "$f" 2>/dev/null | head -3
  echo "-- audio packets --"
  ffprobe -v error -select_streams a:0 -show_entries packet=pts_time -of csv=p=0 "$f" 2>/dev/null | head -3
  # astats / asyncts detection via ffmpeg
  ffmpeg -y -v error -i "$f" -t 3 -af "asetpts=PTS-STARTPTS,aphasemeter=video=0" -f null - 2>&1 | tail -5 || true
done

echo "=== decklink props ==="
gst-inspect-1.0 decklinkvideosrc 2>/dev/null | grep -iE 'timestamp|clock|sync|drop|name |Boolean|Integer' | head -40
echo ----
gst-inspect-1.0 decklinkaudiosrc 2>/dev/null | grep -iE 'timestamp|clock|sync|drop|name |Boolean' | head -30
