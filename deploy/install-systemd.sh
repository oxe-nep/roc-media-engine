#!/usr/bin/env bash
# Install / refresh roc-media-engine systemd unit on the capture host.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
UNIT_SRC="$ROOT/deploy/roc-media-engine.service"
UNIT_DST=/etc/systemd/system/roc-media-engine.service

if [[ ! -f "$ROOT/config.yaml" ]]; then
  cp "$ROOT/config.example.yaml" "$ROOT/config.yaml"
  echo "created $ROOT/config.yaml from example"
fi

echo "Building release with gst feature..."
# shellcheck disable=SC1090
source "$HOME/.cargo/env"
cargo build --release -p roc-engine --features gst

sudo cp "$UNIT_SRC" "$UNIT_DST"
sudo systemctl daemon-reload
sudo systemctl enable roc-media-engine.service
# Drop any stray manual process still holding :8090
sudo fuser -k 8090/tcp >/dev/null 2>&1 || true
sleep 1
sudo systemctl restart roc-media-engine.service
sudo systemctl --no-pager --full status roc-media-engine.service | head -30
