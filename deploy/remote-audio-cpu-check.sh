#!/bin/bash
set -euo pipefail
sleep 25
echo "=== load / cpu ==="
uptime
ps -eo pid,pcpu,pmem,comm --sort=-pcpu | head -6
echo "=== voaacenc count in latest ch1 launch ==="
journalctl -u roc-media-engine --since '5 min ago' --no-pager \
  | grep 'capture pipeline channel=1' | tail -1 \
  | grep -o 'voaacenc' | wc -l
echo "=== channels ==="
python3 - <<'PY'
import json, urllib.request
d = json.load(urllib.request.urlopen("http://127.0.0.1:8090/api/channels", timeout=5))
for c in d["channels"]:
    print(f"ch{c['id']}: {c['status']} srt={c['srt']} br={c.get('srt_bitrate_kbps')}")
PY
echo "=== ffprobe 9101 ==="
timeout 18 ffprobe -v error -analyzeduration 8M -probesize 8M \
  -show_entries stream=index,codec_type,codec_name \
  -of csv=p=0 "srt://127.0.0.1:9101?mode=caller&latency=200" 2>&1 | head -20 || true
