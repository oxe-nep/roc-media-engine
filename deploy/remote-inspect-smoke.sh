#!/usr/bin/env bash
set -u
BASE=http://127.0.0.1:8080
echo '=== channels 3/4 ==='
curl -sS "$BASE/api/channels" | python3 -c '
import json,sys
for c in json.load(sys.stdin)["channels"]:
  if c["id"] in (3,4):
    print(c["id"], c["status"], c.get("input_format"), "bit", c.get("bit_depth"), "mezz", c.get("mezz_label"), "err", c.get("last_error"))
'
UN=/mnt/nep-storage/SHL/files/roc-recording/_unsorted
echo '=== dnx mediainfo ==='
F=$(ls -t "$UN"/smoke_i_dnxhd_ch3_*.mxf 2>/dev/null | head -1)
echo "F=$F"
if [ -n "$F" ]; then
  mediainfo "$F" 2>/dev/null | grep -iE 'Format|Scan|Bit rate|Frame rate|Width|Height|Field|Order|Compression|Duration' | head -40
fi
echo '=== prores ==='
P=$(ls -t "$UN"/smoke_p_prores_ch4_*.mov 2>/dev/null | head -1)
echo "P=$P"
if [ -n "$P" ]; then
  ls -la "$P"
  ffprobe -v error -show_format -show_streams "$P" 2>&1 | head -50
fi
echo '=== proxy ==='
X=$(ls -t "$UN"/smoke_p_proxy_ch4_*_proxy.mp4 2>/dev/null | head -1)
echo "X=$X"
if [ -n "$X" ]; then
  ls -la "$X"
  ffprobe -v error -show_format "$X" 2>&1 | head -20
fi
echo '=== journal (10m) ==='
journalctl -u roc-media-engine --since '12 min ago' --no-pager \
  | grep -iE 'channel.?=3|channel.?=4|mezz|dnx|prores|qtmux|EOS|moov|proxy|signal|waiting|error|WARN' \
  | tail -80
