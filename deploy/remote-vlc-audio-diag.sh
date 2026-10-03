#!/bin/bash
set -euo pipefail
echo "=== service ==="
systemctl is-active roc-media-engine
systemctl show roc-media-engine -p ActiveEnterTimestamp --value
ls -la /opt/applications/roc-media-engine/target/release/roc-media-engine | awk '{print $5,$6,$7,$8,$9}'
echo "=== cpu ==="
ps -eo pcpu,pmem,comm --sort=-pcpu | head -8
echo "=== ch1 status ==="
curl -sf -m 5 http://127.0.0.1:8090/api/channels/1 | python3 -c 'import sys,json; d=json.load(sys.stdin); print({k:d.get(k) for k in ("id","running","srt","srt_url","srt_bitrate_kbps","preset","last_error")})'
echo "=== journal srt ch1 ==="
journalctl -u roc-media-engine --since '30 min ago' --no-pager | grep -E 'channel=1.*(SRT valve|stereo AAC|waiting|error|WARN)' | tail -20
echo "=== ensure listener 9101 ==="
curl -sf -m 10 -X POST http://127.0.0.1:8090/api/channels/1/srt/stop >/dev/null 2>&1 || true
sleep 1
curl -sf -m 30 -X POST -H 'Content-Type: application/json' \
  -d '{"url":"srt://0.0.0.0:9101?mode=listener&latency=120"}' \
  http://127.0.0.1:8090/api/channels/1/srt/start >/dev/null
sleep 3
echo "=== capture ==="
rm -f /tmp/vlc_check.ts
timeout 12 ffmpeg -y -hide_banner -loglevel warning \
  -i 'srt://127.0.0.1:9101?mode=caller&latency=200' -t 5 -c copy /tmp/vlc_check.ts 2>&1 | tail -20
ffprobe -v error -show_entries stream=index,codec_type,codec_name,channels,sample_rate,bit_rate -of json /tmp/vlc_check.ts
echo "=== decode discontinuities ==="
timeout 15 ffmpeg -hide_banner -loglevel info -fflags +genpts \
  -i /tmp/vlc_check.ts -t 5 -f null - 2>&1 \
  | grep -iE 'Stream #|error|Invalid|discontinu|corrupt|queue|aac|Audio|Warning' | head -50
echo "=== pts spacing audio ==="
ffprobe -v error -select_streams a:0 -show_entries packet=pts_time,dts_time,duration_time,size \
  -of csv=p=0 /tmp/vlc_check.ts | head -40
echo "=== size ==="
ls -la /tmp/vlc_check.ts
echo "=== launch snippet ==="
journalctl -u roc-media-engine --since '2 min ago' --no-pager \
  | grep 'capture pipeline channel=1' | tail -1 \
  | python3 -c '
import sys
s=sys.stdin.read()
for part in s.split("!"):
    p=part.strip()
    if any(k in p for k in ("q_srt","srtmux","srt_valve","srt_a","voaacenc","audioconvert","audiorate","audioresample")):
        print("!", p[:220])
'
