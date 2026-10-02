#!/usr/bin/env bash
set -euo pipefail
source "$HOME/.cargo/env"
cd /opt/applications/roc-media-engine
cargo build --release -p roc-engine --features gst
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

# Verify unique REC paths for all 8
for i in 1 2 3 4 5 6 7 8; do
  curl -sf -X POST "http://127.0.0.1:8090/api/channels/$i/record/start?label=uniqtest" >/dev/null
done
sleep 2
curl -sf http://127.0.0.1:8090/api/channels -o /tmp/ch.json
python3 - <<'PY'
import json
d=json.load(open("/tmp/ch.json"))
cs=d.get("channels", d)
paths=[]
for c in sorted(cs, key=lambda x:x["id"]):
    p=c.get("recording_path")
    print(c["id"], c.get("recording"), p)
    paths.append(p)
assert all(c.get("recording") for c in cs), "not all recording"
assert len(set(paths))==8, f"path collision: {paths}"
print("UNIQUE_OK", len(set(paths)))
PY
for i in 1 2 3 4 5 6 7 8; do
  curl -sf -X POST "http://127.0.0.1:8090/api/channels/$i/record/stop" >/dev/null
done
ls -lt /opt/applications/roc-media-engine/recordings/_unsorted/uniqtest_ch*.mp4 | head -20
echo DONE
