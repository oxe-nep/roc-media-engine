#!/usr/bin/env bash
set -euo pipefail
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/encode-preset' \
  -H 'Content-Type: application/json' -d '{"preset":"h_265_10mbit"}'
echo
sleep 5
timeout 5 ffmpeg -y -v error -f mpegts \
  -i 'udp://239.255.28.4:21004?localaddr=127.0.0.1&reuse=1&timeout=3000000' \
  -t 2 -c copy /tmp/ch4_hevc2.ts 2>/tmp/ch4_hevc2.err || true
echo "HEVC_STREAMS:"
ffprobe -v error -show_entries stream=index,codec_name,codec_type -of csv=p=0 /tmp/ch4_hevc2.ts || true
journalctl -u roc-media-engine --since "30 sec ago" --no-pager | grep -E "nvh265enc|with_audio|channel=4" | tail -5
# restore proxy + SRT for player
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/encode-preset' \
  -H 'Content-Type: application/json' -d '{"preset":"proxy"}' >/dev/null
sleep 3
SRT_URL='srt://10.199.23.251:8890?latency=1000000&mode=caller&pkt_size=1316&streamid=publish:live/main:publisher:3fQjvsAHKXE7GAyZ'
curl -s -X POST 'http://127.0.0.1:8090/api/channels/4/srt/start' \
  -H 'Content-Type: application/json' -d "{\"url\":\"$SRT_URL\"}" >/dev/null
curl -s http://127.0.0.1:8090/api/channels/4 | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d["encode_preset"], d["srt"], round(d.get("srt_bitrate_kbps") or 0), d["srt_url"][:60])'
