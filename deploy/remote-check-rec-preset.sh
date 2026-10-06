#!/usr/bin/env bash
set -eu
BASE=http://127.0.0.1:8080
echo '=== streams ==='
curl -sS "$BASE/api/streams" | python3 -c 'import json,sys
for s in json.load(sys.stdin):
  print(s["id"], "enc=", s.get("encode_preset"), "rec=", s.get("record_preset"), s.get("status"))'
echo '=== mezz presets ==='
curl -sS "$BASE/api/encode/presets" | python3 -c 'import json,sys
for p in json.load(sys.stdin):
  c=(p.get("video_codec") or "").lower()
  if "dnx" in c or "prores" in c or "dnx" in p.get("id","") or "prores" in p.get("id",""):
    print(p["id"], p.get("video_codec"), "ch", p.get("audio_channels"))'
echo '=== recent files ==='
ls -lt /opt/applications/roc-media-engine/recordings/_unsorted/ | head -10
echo '=== journal ==='
journalctl -u roc-media-engine --since '45 min ago' --no-pager | grep -iE 'record preset|attached record|attached mezz|dnxhd|prores|start.recording|encode preset' | tail -40
