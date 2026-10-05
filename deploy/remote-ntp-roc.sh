#!/usr/bin/env bash
# Point chrony at ROC LAN NTP servers (10.199.6.10 / 10.199.6.14).
set -euo pipefail

SRC_DIR=/etc/chrony/sources.d
INSTALL_FROM="${1:-}"

if [ -n "$INSTALL_FROM" ] && [ -f "$INSTALL_FROM" ]; then
  sudo cp "$INSTALL_FROM" "$SRC_DIR/roc-ntp.sources"
else
  sudo tee "$SRC_DIR/roc-ntp.sources" >/dev/null <<'EOF'
# ROC local NTP (capture host)
server 10.199.6.10 iburst prefer
server 10.199.6.14 iburst prefer
EOF
fi

# Stop using public Ubuntu NTS pools on this host (prefer facility NTP).
if [ -f "$SRC_DIR/ubuntu-ntp-pools.sources" ]; then
  sudo mv "$SRC_DIR/ubuntu-ntp-pools.sources" "$SRC_DIR/ubuntu-ntp-pools.sources.disabled"
fi

sudo systemctl restart chrony
sleep 2
echo "=== chronyc sources ==="
chronyc sources -v | head -20
echo "=== chronyc tracking ==="
chronyc tracking | head -15
timedatectl status | head -12
