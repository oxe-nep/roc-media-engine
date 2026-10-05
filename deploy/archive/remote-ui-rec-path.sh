#!/usr/bin/env bash
set -euo pipefail
source "$HOME/.cargo/env"

cd /opt/applications/roc-media-engine
# restore fallback recordings_dir if we previously pointed at Go path
python3 - <<'PY'
from pathlib import Path
p = Path("/opt/applications/roc-media-engine/config.yaml")
t = p.read_text()
t2 = t.replace(
    'recordings_dir: "/opt/application/roc-recording/backend/recordings"',
    'recordings_dir: "./recordings"',
)
if t2 != t:
    p.write_text(t2)
    print("config recordings_dir -> ./recordings (fallback)")
else:
    print("config recordings_dir ok")
PY

cargo build --release -p roc-engine --features gst

sudo cp /tmp/me_client.go /opt/application/roc-recording/backend/internal/mediaengine/client.go
sudo cp /tmp/me_routes.go /opt/application/roc-recording/backend/internal/api/routes.go
sudo cp /tmp/me_restore.go /opt/application/roc-recording/backend/internal/bootstrap/restore.go
sudo cp /tmp/me_main.go /opt/application/roc-recording/backend/cmd/server/main.go
sudo cp /tmp/me_rec_manager.go /opt/application/roc-recording/backend/internal/recording/manager.go
sudo sed -i 's/\r$//' \
  /opt/application/roc-recording/backend/internal/mediaengine/client.go \
  /opt/application/roc-recording/backend/internal/api/routes.go \
  /opt/application/roc-recording/backend/internal/bootstrap/restore.go \
  /opt/application/roc-recording/backend/cmd/server/main.go \
  /opt/application/roc-recording/backend/internal/recording/manager.go

cd /opt/application/roc-recording/backend
go build -buildvcs=false -o /tmp/roc-recording-new ./cmd/server
sudo mv /tmp/roc-recording-new ./roc-recording

sudo systemctl stop roc-recording || true
for i in 1 2 3 4 5 6 7 8; do
  curl -s -o /dev/null -X POST "http://127.0.0.1:8090/api/channels/$i/stop" || true
done
sleep 1
sudo systemctl restart roc-media-engine
sleep 2
sudo systemctl start roc-recording
sleep 16
curl -s http://127.0.0.1:8090/health; echo

# Simulate Go/UI path: use RecordingDir equivalent
UI_DIR=$(python3 - <<'PY'
import json,os
p="/opt/application/roc-recording/backend/recordings-path.json"
if os.path.exists(p):
    d=json.load(open(p))
    print(d.get("path") or "/opt/application/roc-recording/backend/recordings")
else:
    print("/opt/application/roc-recording/backend/recordings")
PY
)
echo "UI recordings dir: $UI_DIR"
PATH1="$UI_DIR/_unsorted/uitest_ch1_$(date +%Y-%m-%d_%H-%M-%S)_ch1.mp4"
code=$(curl -s -o /tmp/rb.txt -w "%{http_code}" -X POST "http://127.0.0.1:8090/api/channels/1/record/start?path=$(python3 -c "import urllib.parse; print(urllib.parse.quote('''$PATH1'''))")")
echo "engine path start -> $code $(head -c 200 /tmp/rb.txt)"
sleep 2
curl -s http://127.0.0.1:8090/api/channels/1 | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d.get("recording"), d.get("recording_path"), d.get("last_error"))'
ls -la "$PATH1" 2>/dev/null || ls -la "$(dirname "$PATH1")" | tail -5
curl -sf -X POST "http://127.0.0.1:8090/api/channels/1/record/stop" >/dev/null || true

# Also hit Go API if we can find a key
echo DONE
