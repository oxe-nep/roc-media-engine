#!/bin/bash
set -euo pipefail
echo "=== presets ==="
curl -sf http://127.0.0.1:8090/api/encode/presets | python3 -c '
import sys,json
for p in json.load(sys.stdin).get("presets",[]):
    print(p.get("id"), "ch=", p.get("audio_channels"), "br=", p.get("audio_bitrate"))
'
echo "=== channel status ==="
curl -sf http://127.0.0.1:8090/api/channels | python3 -c '
import sys,json
d=json.load(sys.stdin)
chs=d.get("channels", d if isinstance(d,list) else [])
for c in chs:
    print(c.get("id"), "st", c.get("status"), "srt", c.get("srt"), "br", c.get("srt_bitrate_kbps"), "err", c.get("last_error"), "url", (c.get("srt_url") or "")[:80])
'
echo "=== restart ch1 listener + ch2 mtx ==="
curl -sf -m 10 -X POST http://127.0.0.1:8090/api/channels/1/srt/stop >/dev/null 2>&1 || true
curl -sf -m 10 -X POST http://127.0.0.1:8090/api/channels/2/srt/stop >/dev/null 2>&1 || true
sleep 1
curl -sf -m 30 -X POST -H 'Content-Type: application/json' \
  -d '{"url":"srt://0.0.0.0:9101?mode=listener&latency=120"}' \
  http://127.0.0.1:8090/api/channels/1/srt/start
curl -sf -m 30 -X POST -H 'Content-Type: application/json' \
  -d '{"url":"srt://10.199.23.251:8890?latency=1000000&mode=caller&pkt_size=1316&streamid=publish:live/main:publisher:3fQjvsAHKXE7GAyZ"}' \
  http://127.0.0.1:8090/api/channels/2/srt/start
sleep 5
echo "=== journal ==="
journalctl -u roc-media-engine --since '90 sec ago' --no-pager \
  | grep -E 'SRT valve|stereo AAC|waiting|error|ERROR|channel=[12].*SRT|PMT' | tail -30
echo "=== ch1 launch aac bits ==="
journalctl -u roc-media-engine --since '90 sec ago' --no-pager \
  | grep 'capture pipeline channel=1' | tail -1 | python3 -c '
import sys
s=sys.stdin.read()
print("voaacenc", s.count("voaacenc"))
for part in s.split("!"):
    p=part.strip()
    if any(k in p for k in ("srtmux","srt_valve","q_srt","voaacenc","udpmux","decklinkaudiosrc","leaky")):
        print("!", p[:200])
'
echo "=== ffmpeg local 9101 ==="
rm -f /tmp/vlc_check.ts
timeout 10 ffmpeg -y -hide_banner -loglevel warning \
  -i 'srt://127.0.0.1:9101?mode=caller&latency=200' -t 4 -c copy /tmp/vlc_check.ts 2>&1 | tail -15
ffprobe -v error -show_entries stream=codec_type,codec_name,channels,sample_rate -of csv=p=0 /tmp/vlc_check.ts || true
ls -la /tmp/vlc_check.ts 2>/dev/null || true
echo "=== decode check ==="
timeout 12 ffmpeg -hide_banner -loglevel warning -i /tmp/vlc_check.ts -t 3 -f null - 2>&1 | tail -20 || true
echo "=== mtx publish probe (ffmpeg 8s) ==="
timeout 10 ffmpeg -hide_banner -loglevel warning \
  -f lavfi -i smptebars=size=1280x720:rate=25 -f lavfi -i sine=frequency=1000:sample_rate=48000 \
  -c:v libx264 -tune zerolatency -pix_fmt yuv420p -g 50 -c:a aac -b:a 128k -ac 2 \
  -t 6 -f mpegts \
  'srt://10.199.23.251:8890?mode=caller&latency=1000&pkt_size=1316&streamid=publish:live/main:publisher:3fQjvsAHKXE7GAyZ' \
  2>&1 | tail -20 || true
echo "=== ch2 status after ==="
curl -sf http://127.0.0.1:8090/api/channels/2 | python3 -c 'import sys,json; d=json.load(sys.stdin); print({k:d.get(k) for k in ("srt","srt_bitrate_kbps","last_error","srt_url","status")})'
echo "=== cpu ==="
ps -eo pcpu,comm --sort=-pcpu | head -4
ss -ulnp | grep -E '9101|8890' || true
ss -tlnp | grep -E '9101|8890' || true
