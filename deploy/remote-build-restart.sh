#!/usr/bin/env bash
set -euo pipefail
cd /opt/applications/roc-media-engine
git fetch origin
git reset --hard origin/main
echo "building..."
cargo build -p roc-engine --release --features gst 2>&1
sudo systemctl restart roc-media-engine
sleep 2
systemctl is-active roc-media-engine
curl -sS http://127.0.0.1:8080/api/health; echo
curl -sS http://127.0.0.1:8080/api/encode/presets | python3 -c "import json,sys; ps=json.load(sys.stdin); print([p.get('id') or p.get('name') for p in ps] if isinstance(ps,list) else ps)"
