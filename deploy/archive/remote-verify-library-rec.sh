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
sleep 16
# recover channels stuck in error from previous failed REC
for i in 1 2 3 4 5 6 7 8; do
  curl -s -o /dev/null -X POST "http://127.0.0.1:8090/api/channels/$i/stop" || true
done
sleep 1
for i in 1 2 3 4 5 6 7 8; do
  curl -s -o /dev/null -X POST "http://127.0.0.1:8090/api/channels/$i/start" || true
done
sleep 8
curl -s http://127.0.0.1:8090/health; echo

for i in 1 2 3 4 5 6 7 8; do
  code=$(curl -s -o /tmp/rb.txt -w "%{http_code}" -X POST "http://127.0.0.1:8090/api/channels/$i/record/start?label=libtest&category=_unsorted")
  echo "start $i -> $code $(head -c 80 /tmp/rb.txt)"
done
sleep 2
curl -sf http://127.0.0.1:8090/api/channels -o /tmp/ch.json
python3 - <<'PY'
import json
d=json.load(open("/tmp/ch.json"))
cs=d.get("channels", d)
for c in sorted(cs, key=lambda x:x["id"]):
    print(c["id"], c.get("status"), c.get("recording"), c.get("recording_path"), c.get("last_error"))
paths=[c.get("recording_path") or "" for c in cs]
assert all(c.get("recording") for c in cs), "not all recording"
assert all("_unsorted" in p for p in paths), paths
assert len(set(paths))==8
print("UNIQUE_OK")
PY
ls -lt /opt/application/roc-recording/backend/recordings/_unsorted/libtest_ch*.mp4 | head -10
for i in 1 2 3 4 5 6 7 8; do
  curl -sf -X POST "http://127.0.0.1:8090/api/channels/$i/record/stop" >/dev/null || true
done
curl -s "http://127.0.0.1:8080/api/library/files?category=_unsorted" | python3 -c 'import sys,json; d=json.load(sys.stdin); files=d if isinstance(d,list) else d.get("files",[]); names=[f.get("name","") for f in files]; print("library_count", len(files)); print("libtest", sum(1 for n in names if n.startswith("libtest_ch")));'
echo DONE
