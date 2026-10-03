#!/bin/bash
set -uo pipefail
echo "=== units ==="
systemctl list-units --type=service --all | grep -iE 'roc|record' || true
echo "=== cmdline ==="
ps -eo pid,cmd | grep -E 'roc-recording|roc-media' | grep -v grep || true

# Stop Go backend by unit name if present
for f in /etc/systemd/system/*.service /lib/systemd/system/*.service; do
  if grep -qE 'roc-recording/backend/roc-recording|roc-recording' "$f" 2>/dev/null; then
    if grep -q 'roc-media-engine' "$f" 2>/dev/null; then
      continue
    fi
    echo "FOUND_UNIT $f"
    bn=$(basename "$f" .service)
    sudo systemctl disable --now "$bn" 2>/dev/null || sudo systemctl stop "$bn" 2>/dev/null || true
  fi
done

# Direct kill of known Go binary paths
sudo pkill -f '/opt/application/roc-recording/backend/roc-recording' 2>/dev/null || true
sudo pkill -f '/opt/applications/roc-recording/backend/roc-recording' 2>/dev/null || true
sleep 1
pgrep -a '[r]oc-recording' || echo "go backend stopped"
pgrep -a '[r]oc-media-engine' || echo "engine missing"

echo "=== srt ch2 mtx ==="
curl -sf -m 10 -X POST http://127.0.0.1:8090/api/srt/2/stop >/dev/null 2>&1 || true
curl -sf -m 20 -X PUT -H 'Content-Type: application/json' \
  -d '{"mode":"caller","target":"srt://10.199.23.251:8890?streamid=publish:live/main:publisher:3fQjvsAHKXE7GAyZ","latency_ms":1000}' \
  http://127.0.0.1:8090/api/srt/2 >/dev/null
curl -sf -m 30 -X POST http://127.0.0.1:8090/api/srt/2/start
echo
sleep 3
curl -sf http://127.0.0.1:8090/api/srt/2; echo
echo "=== library ==="
curl -sf http://127.0.0.1:8090/api/library/categories; echo
curl -sf http://127.0.0.1:8090/api/settings/recordings-path; echo
echo "=== ws via python ==="
python3 -c '
import json, urllib.request
# fallback: hit snapshot-equivalent REST
print("streams", len(json.load(urllib.request.urlopen("http://127.0.0.1:8090/api/streams", timeout=5))))
print("srt", len(json.load(urllib.request.urlopen("http://127.0.0.1:8090/api/srt", timeout=5))))
'
# crude websocket with websockets if available
python3 -c '
import asyncio, json
try:
  import websockets
except Exception as e:
  print("ws skip", e); raise SystemExit(0)
async def main():
  async with websockets.connect("ws://127.0.0.1:8090/ws", open_timeout=5) as ws:
    msg = json.loads(await asyncio.wait_for(ws.recv(), timeout=5))
    print("ws", msg.get("type"), len(msg.get("streams",[])), len(msg.get("meters_encode",{})))
asyncio.run(main())
' || true
