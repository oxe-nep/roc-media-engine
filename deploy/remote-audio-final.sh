#!/bin/bash
set -euo pipefail
# shellcheck disable=SC1090
source "$HOME/.cargo/env"
cd /opt/applications/roc-media-engine
# Sync is done by scp from workstation; just build + restart + verify.
cargo build --release -p roc-engine --features gst 2>&1 | tail -12
sudo systemctl restart roc-media-engine
sleep 4
for id in 1 2 3 4 5 6 7 8; do
  curl -sf -m 45 -X POST "http://127.0.0.1:8090/api/channels/${id}/start" >/dev/null 2>&1 || true
done
sleep 14
for id in 1 2 3 4 5 6 7 8; do
  port=$((9100 + id))
  curl -sf -m 10 -X POST "http://127.0.0.1:8090/api/channels/${id}/srt/stop" >/dev/null 2>&1 || true
  sleep 0.5
  curl -sf -m 30 -X POST -H 'Content-Type: application/json' \
    -d "{\"url\":\"srt://0.0.0.0:${port}?mode=listener&latency=120\"}" \
    "http://127.0.0.1:8090/api/channels/${id}/srt/start" >/dev/null 2>&1 || true
done
sleep 12
echo "=== journal ==="
journalctl -u roc-media-engine --since '90 sec ago' --no-pager \
  | grep -E 'SRT valve open|waiting for full|A/V input|voaacenc' | tail -40
echo "=== voaacenc/ch1 ==="
journalctl -u roc-media-engine --since '90 sec ago' --no-pager \
  | grep -m1 'capture pipeline' | grep -o 'voaacenc' | wc -l
echo "=== srt1 ==="
rm -f /tmp/srt_final.ts
timeout 10 ffmpeg -y -hide_banner -loglevel fatal \
  -i 'srt://127.0.0.1:9101?mode=caller&latency=200' -t 4 -c copy /tmp/srt_final.ts || true
ffprobe -v error -show_entries stream=index,codec_type,codec_name -of csv=p=0 /tmp/srt_final.ts || true
echo "=== udp1 ==="
rm -f /tmp/udp_final.ts
timeout 4 gst-launch-1.0 -e udpsrc address=239.255.28.1 port=21001 multicast-iface=eno1 timeout=3000000000 ! \
  filesink location=/tmp/udp_final.ts sync=false 2>/dev/null || true
ffprobe -v error -show_entries stream=codec_type -of csv=p=0 /tmp/udp_final.ts 2>/dev/null | sort | uniq -c || true
echo "=== cpu ==="
uptime
ps -eo pcpu,comm --sort=-pcpu | head -4
