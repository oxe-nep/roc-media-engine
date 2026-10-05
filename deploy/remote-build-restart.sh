#!/usr/bin/env bash
set -euo pipefail
# Non-interactive SSH often lacks cargo on PATH.
if [ -f "$HOME/.cargo/env" ]; then
  # shellcheck source=/dev/null
  source "$HOME/.cargo/env"
fi
export PATH="${HOME}/.cargo/bin:/usr/local/cargo/bin:${PATH}"
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
curl -sS 'http://127.0.0.1:8080/api/library/files?category=_unsorted' | python3 -c "import json,sys; fs=json.load(sys.stdin); print('library', len(fs)); print([f.get('name') for f in fs[:10]])"
