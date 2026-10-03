#!/usr/bin/env bash
set -euo pipefail
source ~/.cargo/env
cd /opt/applications/roc-media-engine
cargo build --release -p roc-engine --features gst 2>&1 | tail -8
sudo systemctl restart roc-media-engine
sleep 2
for i in 1 2 3 4 5 6 7 8; do
  curl -s -X POST "http://127.0.0.1:8090/api/channels/$i/start" >/dev/null || true
done
sleep 12
PORT=19774
OUT=/tmp/srt_av_probe.ts
rm -f "$OUT"
# Local listener capture while engine publishes as listener too — use caller into engine listener.
# Engine default listener is often 9104; start explicit listener on high port for loopback.
curl -s -X POST "http://127.0.0.1:8090/api/channels/4/srt/stop" >/dev/null || true
sleep 1
curl -s -X POST "http://127.0.0.1:8090/api/channels/4/srt/start" \
  -H 'Content-Type: application/json' \
  -d "{\"url\":\"srt://0.0.0.0:${PORT}?mode=listener&latency=1000\"}"
echo
sleep 2
timeout 8 gst-launch-1.0 -e \
  srtsrc uri="srt://127.0.0.1:${PORT}?mode=caller&latency=1000" wait-for-connection=true ! \
  filesink location="$OUT" sync=false 2>/tmp/srt_gst_err.txt || true
ls -la "$OUT" || true
echo '=== ffprobe ==='
ffprobe -hide_banner "$OUT" 2>&1 | head -25 || true
echo '=== streams ==='
ffprobe -v error -show_entries stream=index,codec_type,codec_name -of csv=p=0 "$OUT" 2>&1 || true
echo '=== engine log ==='
journalctl -u roc-media-engine --since '1 min ago' --no-pager | grep -iE 'srt|AAC|with_audio|ERROR' | tail -15
curl -s -X POST "http://127.0.0.1:8090/api/channels/4/srt/stop" >/dev/null || true
