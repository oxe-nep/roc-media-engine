#!/usr/bin/env bash
set -euo pipefail
UI=$(python3 - <<'PY'
import json,os
p="/opt/application/roc-recording/backend/recordings-path.json"
if os.path.exists(p):
    d=json.load(open(p))
    print(d.get("path") or "/opt/application/roc-recording/backend/recordings")
else:
    print("/opt/application/roc-recording/backend/recordings")
PY
)
echo "UI dir=$UI"
echo "engine user=$(ps -o user= -C roc-media-engine | head -1)"
TS=$(date +%Y-%m-%d_%H-%M-%S)
P="$UI/_unsorted/uitest_ch1_${TS}_ch1.mp4"
ENC=$(python3 -c "import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1]))" "$P")
code=$(curl -s -o /tmp/rb.txt -w "%{http_code}" -X POST "http://127.0.0.1:8090/api/channels/1/record/start?path=$ENC")
echo "start=$code body=$(head -c 220 /tmp/rb.txt)"
sleep 2
python3 - <<'PY'
import json,urllib.request
d=json.load(urllib.request.urlopen("http://127.0.0.1:8090/api/channels/1"))
print("recording", d.get("recording"))
print("path", d.get("recording_path"))
print("err", d.get("last_error"))
PY
ls -la "$P" || true
curl -sf -X POST "http://127.0.0.1:8090/api/channels/1/record/stop" >/dev/null || true
ls -la "$P" || true
echo DONE
