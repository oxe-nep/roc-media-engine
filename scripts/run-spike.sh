#!/usr/bin/env bash
# Run Fas 0 soak on capture host (default 60s).
set -euo pipefail
DEVICE="${1:-DeckLink IP 100G (1)}"
SECS="${2:-60}"
cargo run -p spike-decklink-nvenc --release -- \
  --device "$DEVICE" \
  --output "/tmp/roc-spike-$(date +%Y%m%d-%H%M%S).mp4" \
  --preview \
  --duration-secs "$SECS"
