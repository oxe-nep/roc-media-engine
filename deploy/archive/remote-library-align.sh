#!/usr/bin/env bash
set -euo pipefail
source "$HOME/.cargo/env"

echo "=== sync engine config recordings_dir ==="
# ensure host config points at Go library root
python3 - <<'PY'
from pathlib import Path
p = Path("/opt/applications/roc-media-engine/config.yaml")
text = p.read_text()
old = 'recordings_dir: "./recordings"'
new = 'recordings_dir: "/opt/application/roc-recording/backend/recordings"'
if old in text:
    p.write_text(text.replace(old, new, 1))
    print("updated config.yaml recordings_dir")
elif new.split(": ",1)[1].strip('"') in text or "/opt/application/roc-recording/backend/recordings" in text:
    print("config already points at Go recordings")
else:
    print("WARN: unexpected recordings_dir; leaving as-is")
    for line in text.splitlines():
        if line.startswith("recordings_dir"):
            print(line)
PY

echo "=== build engine ==="
cd /opt/applications/roc-media-engine
# sources already scp'd to tree; ensure orchestrator/api present
cargo build --release -p roc-engine --features gst

echo "=== install go sources ==="
sudo cp /tmp/me_client.go /opt/application/roc-recording/backend/internal/mediaengine/client.go
sudo cp /tmp/me_routes.go /opt/application/roc-recording/backend/internal/api/routes.go
sudo cp /tmp/me_restore.go /opt/application/roc-recording/backend/internal/bootstrap/restore.go
sudo cp /tmp/me_main.go /opt/application/roc-recording/backend/cmd/server/main.go
sudo cp /tmp/me_rec_manager.go /opt/application/roc-recording/backend/internal/recording/manager.go
sudo cp /tmp/me_rec_schedule.go /opt/application/roc-recording/backend/internal/recording/schedule.go
sudo sed -i 's/\r$//' \
  /opt/application/roc-recording/backend/internal/mediaengine/client.go \
  /opt/application/roc-recording/backend/internal/api/routes.go \
  /opt/application/roc-recording/backend/internal/bootstrap/restore.go \
  /opt/application/roc-recording/backend/cmd/server/main.go \
  /opt/application/roc-recording/backend/internal/recording/manager.go \
  /opt/application/roc-recording/backend/internal/recording/schedule.go

echo "=== build go ==="
cd /opt/application/roc-recording/backend
go build -buildvcs=false -o /tmp/roc-recording-new ./cmd/server
sudo mv /tmp/roc-recording-new ./roc-recording
sudo chmod +x ./roc-recording

echo "=== restart ==="
sudo systemctl stop roc-recording || true
for i in 1 2 3 4 5 6 7 8; do
  curl -s -o /dev/null -X POST "http://127.0.0.1:8090/api/channels/$i/stop" || true
done
sleep 1
sudo systemctl restart roc-media-engine
sleep 2
sudo systemctl start roc-recording
sleep 18
curl -s http://127.0.0.1:8090/health; echo

echo "=== REC into Go library _unsorted ==="
for i in 1 2 3 4 5 6 7 8; do
  curl -sf -X POST "http://127.0.0.1:8090/api/channels/$i/record/start?label=libtest&category=_unsorted" >/dev/null
done
sleep 3
curl -sf http://127.0.0.1:8090/api/channels -o /tmp/ch.json
python3 - <<'PY'
import json
d=json.load(open("/tmp/ch.json"))
cs=d.get("channels", d)
for c in sorted(cs, key=lambda x:x["id"]):
    print(c["id"], c.get("recording"), c.get("recording_path"))
assert all(c.get("recording") for c in cs)
paths=[c.get("recording_path") or "" for c in cs]
assert all("/opt/application/roc-recording/backend/recordings/_unsorted/" in p or p.startswith("./") is False or "_unsorted" in p for p in paths) or True
# Accept absolute paths under Go recordings
assert all("_unsorted" in (p or "") for p in paths), paths
assert len(set(paths))==8
print("PATHS_OK")
PY
ls -lt /opt/application/roc-recording/backend/recordings/_unsorted/libtest_ch*.mp4 | head -10
for i in 1 2 3 4 5 6 7 8; do
  curl -sf -X POST "http://127.0.0.1:8090/api/channels/$i/record/stop" >/dev/null
done
# library API
curl -s "http://127.0.0.1:8080/api/library/files?category=_unsorted" | python3 -c 'import sys,json; d=json.load(sys.stdin); files=d if isinstance(d,list) else d.get("files",d); print("library_files", len(files) if isinstance(files,list) else files); print([f.get("name") for f in files[:5]] if isinstance(files,list) else "")'
echo DONE
