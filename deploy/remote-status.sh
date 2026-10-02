#!/usr/bin/env bash
set -euo pipefail
curl -s http://127.0.0.1:8090/api/encode/presets > /tmp/presets.json
python3 - <<'PY'
import json
d=json.load(open("/tmp/presets.json"))
for p in d.get("presets",[]):
    print(f"{p['id']}: {p['video_codec']} {p['video_bitrate']} preset={p.get('video_preset')}")
PY
echo "=== ch4 ==="
curl -s http://127.0.0.1:8090/api/channels/4
echo
echo "=== HLS EXTINF sample ==="
grep EXTINF /opt/application/roc-recording/backend/hls/4/preview.m3u8 | head -8
echo "=== all channel status ==="
for i in 1 2 3 4 5 6 7 8; do
  s=$(curl -s "http://127.0.0.1:8090/api/channels/$i")
  python3 -c "import json,sys; d=json.loads(sys.argv[1]); print(d['id'], d['status'], d['encode_preset'], 'srt='+str(d['srt']), 'kbps='+str(round(d.get('video_bitrate_kbps') or 0)))" "$s"
done
