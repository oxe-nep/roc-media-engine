#!/bin/bash
set -euo pipefail
cd /opt/applications/roc-media-engine
cp -f /tmp/config.yaml config.yaml
# Keep example in sync if present
if [[ -f /tmp/config.example.yaml ]]; then
  cp -f /tmp/config.example.yaml config.example.yaml
fi
grep -E 'audio_channels|^  (proxy|hq|mezz)' config.yaml | head -20
sudo systemctl restart roc-media-engine
sleep 5
for id in 1 2 3 4 5 6 7 8; do
  curl -sf -m 45 -X POST "http://127.0.0.1:8090/api/channels/${id}/start" >/dev/null 2>&1 || true
done
sleep 8
echo "=== ch1 ==="
curl -sf -m 5 http://127.0.0.1:8090/api/channels/1 | python3 -c '
import sys,json
d=json.load(sys.stdin)
print("srt", d.get("srt"), "url", d.get("srt_url"))
print("keys", sorted(d.keys())[:40])
'
echo "=== journal audio_channels / AAC ==="
journalctl -u roc-media-engine --since '60 sec ago' --no-pager \
  | grep -E 'capture pipeline channel=1|audio_channels|voaacenc|REC video-only' | tail -15 \
  | python3 -c '
import sys
for line in sys.stdin:
    if "capture pipeline channel=1" in line:
        s=line
        print("voaacenc count", s.count("voaacenc"))
        print("channels=8" in s, "channels=2" in s or "channels=2," in s)
        for part in s.split("!"):
            p=part.strip()
            if "decklinkaudiosrc" in p or "voaacenc" in p or "channels=" in p:
                print("!", p[:180])
    else:
        print(line.rstrip()[:200])
'
