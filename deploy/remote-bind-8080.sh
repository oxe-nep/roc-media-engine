#!/bin/bash
set -euo pipefail
cd /opt/applications/roc-media-engine
cp -f /tmp/config.yaml config.yaml
# Prefer config bind; clear unit override if it forces 8090
if [[ -f /etc/roc-media-engine.env ]]; then
  sudo sed -i 's/8090/8080/g' /etc/roc-media-engine.env || true
fi
if [[ -f /etc/systemd/system/roc-media-engine.service ]]; then
  if grep -q '8090' /etc/systemd/system/roc-media-engine.service; then
    sudo sed -i 's/8090/8080/g' /etc/systemd/system/roc-media-engine.service
    sudo systemctl daemon-reload
  fi
fi
# Go must stay down so :8080 is free
sudo systemctl disable --now roc-recording 2>/dev/null || true
sudo pkill -f '/opt/application/roc-recording/backend/roc-recording' 2>/dev/null || true
sleep 1
sudo systemctl restart roc-media-engine
sleep 4
ss -tlnp | grep -E ':8080|:8090' || true
systemctl is-active roc-media-engine
systemctl is-active roc-recording || echo 'roc-recording inactive OK'
curl -sf http://127.0.0.1:8080/api/health
echo
curl -sf http://127.0.0.1:8080/api/streams | python3 -c 'import sys,json; d=json.load(sys.stdin); print("streams", len(d))'
