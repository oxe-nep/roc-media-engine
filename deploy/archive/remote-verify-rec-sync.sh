#!/usr/bin/env bash
set -euo pipefail
OUT=/tmp/rec_sync_test8.mp4
rm -f "$OUT"
sudo systemctl restart roc-media-engine
sleep 2
for i in 1 2 3 4 5 6 7 8; do
  curl -s -X POST "http://127.0.0.1:8090/api/channels/$i/start" >/dev/null || true
done
sleep 10
curl -s http://127.0.0.1:8090/health
echo
curl -s -X POST "http://127.0.0.1:8090/api/channels/4/record/start?path=$OUT"
echo
sleep 8
curl -s -m 20 -X POST 'http://127.0.0.1:8090/api/channels/4/record/stop'
echo
sleep 2
journalctl -u roc-media-engine --since '2 min ago' --no-pager | grep -E 'mux A/V|attached record|detached record|opened AAC' | tail -15
echo '==='
ffprobe -hide_banner "$OUT" 2>&1 | head -25
echo '==='
python3 - <<PY
import subprocess, json
r=subprocess.check_output([
  "ffprobe","-v","error",
  "-show_entries","stream=index,codec_type,codec_name,start_time,duration,bit_rate",
  "-of","json","$OUT",
])
d=json.loads(r)
for s in d["streams"]:
    print(s)
vs=next(s for s in d["streams"] if s["codec_type"]=="video")
aus=next(s for s in d["streams"] if s["codec_type"]=="audio")
sd=(float(aus["start_time"])-float(vs["start_time"]))*1000
dd=(float(aus["duration"])-float(vs["duration"]))*1000
print(f"start_delta_ms={sd:.3f}")
print(f"duration_delta_ms={dd:.3f}")
print(f"size_bytes={subprocess.check_output(['stat','-c','%s','$OUT']).decode().strip()}")
if abs(sd) < 50 and abs(dd) < 200:
    print("PASS: A/V start+duration within tolerance")
else:
    print("CHECK: deltas outside tight tolerance (start<50ms, dur<200ms)")
PY
