#!/bin/bash
set -euo pipefail
echo "=== engine presets ==="
curl -sf http://127.0.0.1:8090/api/encode/presets | python3 -c '
import sys, json
d = json.load(sys.stdin)
for p in d.get("presets", []):
    print(p.get("id"), "audio_channels=", p.get("audio_channels"))
'
echo "=== config.yaml ==="
grep -E 'audio_channels|^  (proxy|hq|mezz)' /opt/applications/roc-media-engine/config.yaml | head -20
echo "=== ch1 launch ==="
journalctl -u roc-media-engine --since "3 min ago" --no-pager \
  | grep "capture pipeline channel=1" | tail -1 | python3 -c '
import sys
s = sys.stdin.read()
print("voaacenc", s.count("voaacenc"))
print("has srtmux", "srtmux" in s)
print("has prog tee/4pair", "prog_a_tee" in s or "prog_aac1" in s)
print("has single srt/prog path", "srtmux" in s or "udpmux" in s)
'
echo "=== go encode-presets.json ==="
find /opt/applications /home/oxe -name "encode-presets.json" 2>/dev/null | while read -r path; do
  echo "FILE $path"
  python3 -c '
import json,sys
path=sys.argv[1]
with open(path) as f: data=json.load(f)
changed=False
def fix(obj):
    global changed
    if isinstance(obj, dict):
        if "audio_channels" in obj and obj["audio_channels"] != 2:
            obj["audio_channels"]=2
            changed=True
        for v in obj.values(): fix(v)
    elif isinstance(obj, list):
        for v in obj: fix(v)
fix(data)
if changed:
    with open(path,"w") as f: json.dump(data,f,indent=2)
    print("patched to stereo")
else:
    print("ok/no-8ch")
print(json.dumps(data)[:400])
' "$path"
done
echo "=== cpu ==="
ps -eo pcpu,comm --sort=-pcpu | head -4
