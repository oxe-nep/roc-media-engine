#!/bin/bash
set -uo pipefail
echo "=== engine pid/uptime ==="
systemctl show roc-media-engine -p ActiveEnterTimestamp -p MainPID --value
ps -o pid,etime,pcpu,cmd -C roc-media-engine 2>/dev/null | head -3
echo "=== ch1 launch SRT bits ==="
journalctl -u roc-media-engine --since '40 min ago' --no-pager \
  | grep 'capture pipeline channel=1' | tail -1 \
  | python3 -c '
import sys,re
s=sys.stdin.read()
for part in s.split("!"):
    p=part.strip()
    if any(k in p for k in ("srt","voaacenc","udpmux","prog_aac","srtmux","q_srt")):
        print("!", p[:220])
'
echo "=== restart ch1 listener clean ==="
curl -sf -m 10 -X POST http://127.0.0.1:8090/api/channels/1/srt/stop >/dev/null || true
sleep 1
curl -sf -m 30 -X POST -H 'Content-Type: application/json' \
  -d '{"url":"srt://0.0.0.0:9101?mode=listener&latency=120"}' \
  http://127.0.0.1:8090/api/channels/1/srt/start
sleep 3
journalctl -u roc-media-engine --since '20 sec ago' --no-pager \
  | grep -E 'channel=1.*(valve open|waiting|stereo)' | tail -10
echo "=== record 3s and probe audio ==="
rm -f /tmp/vlc_check.ts
timeout 6 ffmpeg -y -hide_banner -loglevel warning \
  -i 'srt://127.0.0.1:9101?mode=caller&latency=200' -t 3 -c copy /tmp/vlc_check.ts
ffprobe -v error -show_entries stream=index,codec_type,codec_name -of csv=p=0 /tmp/vlc_check.ts
ffmpeg -hide_banner -loglevel info -i /tmp/vlc_check.ts -vn -af astats=metadata=1:reset=0 -f null - 2>&1 \
  | grep -iE 'error|discard|concealed|RMS|Peak|Stream #0' | head -20
echo "=== mediamtx host ==="
ping -c 1 -W 1 10.199.23.251 >/dev/null && echo ping_ok || echo ping_fail
# UDP port open does not prove process; try SRT handshake
timeout 3 ffmpeg -hide_banner -loglevel warning \
  -i 'srt://10.199.23.251:8890?mode=caller&latency=1000&streamid=publish:live/main:publisher:3fQjvsAHKXE7GAyZ' \
  -t 1 -f null - 2>&1 | tail -8 || true
# any known mtx hosts / containers
docker ps -a --format '{{.Names}} {{.Status}}' 2>/dev/null | head -20
getent hosts mediamtx mtx 2>/dev/null || true
