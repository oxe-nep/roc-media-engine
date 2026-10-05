#!/usr/bin/env bash
set -euo pipefail
source "$HOME/.cargo/env"

echo "=== build media-engine ==="
cd /opt/applications/roc-media-engine
cargo build --release -p roc-engine --features gst

echo "=== install go sources ==="
sudo cp /tmp/me_client.go /opt/application/roc-recording/backend/internal/mediaengine/client.go
sudo cp /tmp/me_dashboard_ws.go /opt/application/roc-recording/backend/internal/api/dashboard_ws.go
sudo cp /tmp/me_routes.go /opt/application/roc-recording/backend/internal/api/routes.go

echo "=== build go backend ==="
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

echo "=== meters api ==="
curl -s http://127.0.0.1:8090/api/meters
echo
curl -s http://127.0.0.1:8080/audio/1
echo
curl -s http://127.0.0.1:8090/health
echo
