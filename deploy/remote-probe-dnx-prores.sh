#!/usr/bin/env bash
# Focused ch3 DNxHD + ch4 ProRes after signal-lock fix.
set -eu
BASE=http://127.0.0.1:8080
UN=/mnt/nep-storage/SHL/files/roc-recording/_unsorted

cleanup() {
  curl -sS -X POST "$BASE/api/recordings/3/hq/stop" >/dev/null 2>&1 || true
  curl -sS -X POST "$BASE/api/recordings/4/hq/stop" >/dev/null 2>&1 || true
  curl -sS -X POST "$BASE/api/recordings/4/proxy/stop" >/dev/null 2>&1 || true
}
trap cleanup EXIT

curl -sS -X PUT "$BASE/api/streams/3/encode-preset" -H 'Content-Type: application/json' -d '{"preset":"hq"}' >/dev/null
curl -sS -X PUT "$BASE/api/streams/3/record-preset" -H 'Content-Type: application/json' -d '{"preset":"dnxhd_hq"}' >/dev/null
curl -sS -X PUT "$BASE/api/streams/4/encode-preset" -H 'Content-Type: application/json' -d '{"preset":"hq"}' >/dev/null
curl -sS -X PUT "$BASE/api/streams/4/record-preset" -H 'Content-Type: application/json' -d '{"preset":"prores_proxy"}' >/dev/null
curl -sS -X POST "$BASE/api/streams/3/stop" >/dev/null 2>&1 || true
curl -sS -X POST "$BASE/api/streams/4/stop" >/dev/null 2>&1 || true
sleep 1
curl -sS -X POST "$BASE/api/streams/3/start" >/dev/null
curl -sS -X POST "$BASE/api/streams/4/start" >/dev/null
sleep 8
curl -sS "$BASE/api/channels" | python3 -c '
import json,sys
for c in json.load(sys.stdin)["channels"]:
  if c["id"] in (3,4):
    print(c["id"], c["status"], c.get("input_format"), "locked", c.get("locked_mode"), "mezz", c.get("mezz_label"))
'

echo "=== DNxHD ch3 ==="
curl -sS -X PUT "$BASE/api/recordings/3/name" -H 'Content-Type: application/json' -d '{"name":"probe_i_dnx"}' >/dev/null
curl -sS -X PUT "$BASE/api/recordings/3/category" -H 'Content-Type: application/json' -d '{"category":"_unsorted"}' >/dev/null
curl -sS -X POST "$BASE/api/recordings/3/hq/start" | head -c 200; echo
sleep 12
curl -sS -w " HTTP:%{http_code}\n" -X POST "$BASE/api/recordings/3/hq/stop"
F=$(ls -t "$UN"/probe_i_dnx_ch3_*.mxf 2>/dev/null | head -1 || true)
echo "F=$F"
if [ -n "$F" ]; then
  ls -la "$F"
  ffprobe -v error -show_entries stream=codec_name,field_order,r_frame_rate -show_entries format=duration,size -of default=nw=1 "$F" 2>&1 | head -20
  mediainfo --Inform="Video;Scan=%ScanType% BR=%BitRate% FPS=%FrameRate%\n" "$F" 2>&1 || true
fi

echo "=== ProRes ch4 ==="
curl -sS -X PUT "$BASE/api/recordings/4/name" -H 'Content-Type: application/json' -d '{"name":"probe_p_prores"}' >/dev/null
curl -sS -X PUT "$BASE/api/recordings/4/category" -H 'Content-Type: application/json' -d '{"category":"_unsorted"}' >/dev/null
curl -sS -X POST "$BASE/api/recordings/4/hq/start" | head -c 200; echo
sleep 15
curl -sS -w " HTTP:%{http_code}\n" -X POST "$BASE/api/recordings/4/hq/stop"
P=$(ls -t "$UN"/probe_p_prores_ch4_*.mov 2>/dev/null | head -1 || true)
echo "P=$P"
if [ -n "$P" ]; then
  ls -la "$P"
  ffprobe -v error -show_entries stream=codec_name,nb_frames,duration,r_frame_rate -show_entries format=duration,size -of default=nw=1 "$P" 2>&1 | head -30
fi

echo "=== journal ==="
journalctl -u roc-media-engine --since '3 min ago' --no-pager \
  | grep -iE 'channel=?[34]|mezz|dnx|prores|EOS|moov|not-negot|WARN|error' \
  | grep -v 'launch=' | tail -40
