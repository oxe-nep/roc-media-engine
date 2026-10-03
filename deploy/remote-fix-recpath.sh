#!/bin/bash
set -euo pipefail
curl -sf -X PUT -H 'Content-Type: application/json' \
  -d '{"path":"/opt/application/roc-recording/backend/recordings"}' \
  http://127.0.0.1:8090/api/settings/recordings-path
echo
curl -sf http://127.0.0.1:8090/api/library/categories | python3 -c 'import sys,json; d=json.load(sys.stdin); print("cats", len(d), [c["name"] for c in d[:8]])'
# Ensure Go stays down
systemctl is-active roc-recording || echo "roc-recording inactive OK"
systemctl is-enabled roc-recording || echo "roc-recording disabled OK"
# HLS reachable
curl -sf -o /dev/null -w "hls_preview %{http_code}\n" http://127.0.0.1:8090/hls/1/preview.m3u8 || echo "hls fail"
# SRT ch1 still stereo
curl -sf http://127.0.0.1:8090/api/srt/1 | python3 -c 'import sys,json; d=json.load(sys.stdin); print("ch1", d.get("status"), d.get("sending"), d.get("bitrate_kbps"))'
curl -sf http://127.0.0.1:8090/api/srt/2 | python3 -c 'import sys,json; d=json.load(sys.stdin); print("ch2 mtx", d.get("status"), d.get("sending"), d.get("bitrate_kbps"))'
# Sync host config.yaml recordings_dir
python3 - <<'PY'
from pathlib import Path
p = Path("/opt/applications/roc-media-engine/config.yaml")
t = p.read_text()
old = 'recordings_dir: "/opt/applications/roc-recording/backend/recordings"'
new = 'recordings_dir: "/opt/application/roc-recording/backend/recordings"'
if old in t:
    p.write_text(t.replace(old, new))
    print("config.yaml patched")
elif new in t:
    print("config.yaml already ok")
else:
    print("config.yaml recordings_dir unknown")
PY
