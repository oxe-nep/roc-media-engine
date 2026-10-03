#!/bin/bash
set -euo pipefail
# shellcheck disable=SC1090
source "$HOME/.cargo/env"
cd /opt/applications/roc-media-engine
cp -f /tmp/describe.rs crates/roc-pipelines/src/describe.rs
cp -f /tmp/capture.rs crates/roc-pipelines/src/gst/capture.rs
cargo build --release -p roc-engine --features gst 2>&1 | tail -10
sudo systemctl restart roc-media-engine
sleep 5
for id in 1 2 3 4 5 6 7 8; do
  curl -sf -m 45 -X POST "http://127.0.0.1:8090/api/channels/${id}/start" >/dev/null 2>&1 || true
done
sleep 12
curl -sf -m 10 -X POST "http://127.0.0.1:8090/api/channels/1/srt/stop" >/dev/null 2>&1 || true
sleep 1
curl -sf -m 30 -X POST -H 'Content-Type: application/json' \
  -d '{"url":"srt://0.0.0.0:9101?mode=listener&latency=120"}' \
  "http://127.0.0.1:8090/api/channels/1/srt/start"
sleep 4
echo "=== binary/mtime ==="
ls -la target/release/roc-media-engine
systemctl show roc-media-engine -p ActiveEnterTimestamp --value
echo "=== ch1 SRT launch ==="
journalctl -u roc-media-engine --since '90 sec ago' --no-pager \
  | grep 'capture pipeline channel=1' | tail -1 \
  | python3 -c '
import sys
s=sys.stdin.read()
for part in s.split("!"):
    p=part.strip()
    if any(k in p for k in ("q_srt","srtmux","srt_valve","srt_a","voaacenc")):
        print("!", p[:200])
'
echo "=== valve ==="
journalctl -u roc-media-engine --since '90 sec ago' --no-pager \
  | grep -E 'channel=1.*(valve open|waiting)' | tail -8
echo "=== ffmpeg 9101 ==="
rm -f /tmp/vlc_check.ts
timeout 8 ffmpeg -y -hide_banner -loglevel warning \
  -i 'srt://127.0.0.1:9101?mode=caller&latency=200' -t 3 -c copy /tmp/vlc_check.ts
ffprobe -v error -show_entries stream=codec_type,codec_name -of csv=p=0 /tmp/vlc_check.ts
ls -la /tmp/vlc_check.ts
echo "=== mediamtx ==="
ping -c 1 -W 1 10.199.23.251 >/dev/null && echo ping_ok || echo ping_fail
timeout 3 ffmpeg -hide_banner -loglevel warning \
  -i 'srt://10.199.23.251:8890?mode=caller&latency=1000&streamid=publish:live/main:publisher:3fQjvsAHKXE7GAyZ' \
  -t 1 -f null - 2>&1 | tail -6 || true
ss -ulnp | grep 8890 || echo 'no local :8890'
pgrep -a mediamtx || echo 'no mediamtx process on roc-capture'
