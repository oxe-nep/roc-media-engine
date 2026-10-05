#!/usr/bin/env bash
set -euo pipefail

APP=/opt/applications/roc-media-engine
LEGACY_REC=/opt/application/roc-recording/backend/recordings
LEGACY_HLS=/opt/applications/roc-recording/backend/hls

echo "=== before ==="
ls -lad "$APP/recordings" "$APP/hls" 2>&1 || true
ls -lad "$LEGACY_REC" "$LEGACY_HLS"

# recordings -> symlink to legacy library
if [ -L "$APP/recordings" ]; then
  echo "recordings already symlink -> $(readlink "$APP/recordings")"
elif [ -d "$APP/recordings" ]; then
  count=$(find "$APP/recordings" -mindepth 1 | wc -l)
  if [ "$count" -gt 0 ]; then
    sudo mv "$APP/recordings" "$APP/recordings.local-bak-$(date +%Y%m%d%H%M%S)"
  else
    sudo rm -rf "$APP/recordings"
  fi
  sudo ln -sfn "$LEGACY_REC" "$APP/recordings"
  echo "linked recordings -> $LEGACY_REC"
else
  sudo ln -sfn "$LEGACY_REC" "$APP/recordings"
  echo "linked recordings -> $LEGACY_REC"
fi

# hls -> symlink to existing live tree
if [ -L "$APP/hls" ]; then
  echo "hls already symlink -> $(readlink "$APP/hls")"
elif [ -e "$APP/hls" ]; then
  echo "hls path exists, leaving as-is: $(ls -lad "$APP/hls")"
else
  sudo ln -sfn "$LEGACY_HLS" "$APP/hls"
  echo "linked hls -> $LEGACY_HLS"
fi

python3 <<'PY'
from pathlib import Path

p = Path("/opt/applications/roc-media-engine/config.yaml")
text = p.read_text()
repls = {
    'recordings_dir: "/opt/application/roc-recording/backend/recordings"':
        'recordings_dir: "/opt/applications/roc-media-engine/recordings"',
    'hls_dir: "/opt/applications/roc-recording/backend/hls"':
        'hls_dir: "/opt/applications/roc-media-engine/hls"',
}
for old, new in repls.items():
    if old in text:
        text = text.replace(old, new)
        print("replaced", old.split(":")[0])
    elif new in text:
        print("already", new.split(":")[0])
    else:
        print("WARN missing", old)
Path("/tmp/rme-config.yaml").write_text(text)
PY
sudo cp /tmp/rme-config.yaml "$APP/config.yaml"

python3 <<'PY'
import json
from pathlib import Path

p = Path("/opt/applications/roc-media-engine/ui-state.json")
d = json.loads(p.read_text())
old = d.get("recordings_dir")
d["recordings_dir"] = "/opt/applications/roc-media-engine/recordings"
Path("/tmp/rme-ui-state.json").write_text(json.dumps(d, indent=2) + "\n")
print("ui-state recordings_dir", old, "->", d["recordings_dir"])
PY
sudo cp /tmp/rme-ui-state.json "$APP/ui-state.json"

echo "=== config paths ==="
grep -E "recordings_dir|hls_dir" "$APP/config.yaml"
echo "=== links ==="
ls -lad "$APP/recordings" "$APP/hls"

sudo systemctl restart roc-media-engine
sleep 2
systemctl is-active roc-media-engine
curl -sS http://127.0.0.1:8080/api/health
echo
curl -sS http://127.0.0.1:8080/api/library/categories
echo
