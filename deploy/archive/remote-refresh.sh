#!/usr/bin/env bash
set -euo pipefail
ROOT=/opt/applications/roc-media-engine
BIN=$ROOT/target/release/roc-media-engine
pids=$(pgrep -f "$BIN" || true)
if [[ -n "${pids}" ]]; then
  echo Stopping: $pids
  kill $pids || true
  sleep 2
fi
cd "$ROOT"
# drop local dirty copy if present
rm -f deploy/install-systemd.sh
git pull
source "$HOME/.cargo/env"
cargo build --release -p roc-engine --features gst
chmod +x deploy/install-systemd.sh
./deploy/install-systemd.sh
curl -s http://127.0.0.1:8090/health || true
echo
