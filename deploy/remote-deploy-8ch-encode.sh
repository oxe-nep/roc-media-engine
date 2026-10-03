#!/bin/bash
set -euo pipefail
# shellcheck disable=SC1090
source "$HOME/.cargo/env"
cd /opt/applications/roc-media-engine

echo "=== config audio_channels ==="
grep -E 'audio_channels|^  (proxy|hq|mezz)' config.yaml | head -20

echo "=== unit test ==="
cargo test -p roc-pipelines four_aac -- --nocapture

echo "=== build ==="
cargo build --release -p roc-engine --features gst

echo "=== restart ==="
sudo systemctl restart roc-media-engine
sleep 4
systemctl is-active roc-media-engine

echo "=== start capture + SRT listeners ==="
for id in 1 2 3 4 5 6 7 8; do
  curl -sf -m 45 -X POST "http://127.0.0.1:8090/api/channels/${id}/start" >/dev/null 2>&1 || true
done
sleep 10
for id in 1 2 3 4 5 6 7 8; do
  port=$((9100 + id))
  curl -sf -m 10 -X POST "http://127.0.0.1:8090/api/channels/${id}/srt/stop" >/dev/null 2>&1 || true
  sleep 1
  curl -sf -m 30 -X POST -H 'Content-Type: application/json' \
    -d "{\"url\":\"srt://0.0.0.0:${port}?mode=listener&latency=120\"}" \
    "http://127.0.0.1:8090/api/channels/${id}/srt/start" >/dev/null 2>&1 || true
done
sleep 12

echo "=== journal (valve / pairs) ==="
journalctl -u roc-media-engine --since '90 sec ago' --no-pager \
  | grep -iE 'audio_pairs|SRT A/V|SRT valve open|waiting for full|baked|ERROR' \
  | tail -40

echo "=== probe streams (expect video + 4 aac) ==="
python3 - <<'PY'
import json, subprocess, urllib.request
d = json.load(urllib.request.urlopen("http://127.0.0.1:8090/api/channels", timeout=5))
ok = 0
for c in d["channels"]:
    cid = c["id"]
    port = 9100 + cid
    p = subprocess.run(
        [
            "timeout", "15", "ffprobe", "-v", "error",
            "-analyzeduration", "5M", "-probesize", "5M",
            "-i", f"srt://127.0.0.1:{port}?mode=caller&latency=200",
            "-show_entries", "stream=index,codec_type,codec_name",
            "-of", "csv=p=0",
        ],
        capture_output=True, text=True,
    )
    lines = [ln.strip() for ln in p.stdout.splitlines() if ln.strip()]
    n_v = sum(1 for ln in lines if ",video," in ln or ln.endswith(",video"))
    # csv: index,codec_type,codec_name OR codec_type,codec_name
    n_a = sum(1 for ln in lines if "audio" in ln.split(","))
    # also count aac specifically
    n_aac = sum(1 for ln in lines if "aac" in ln)
    good = n_v >= 1 and n_a >= 4
    if good:
        ok += 1
    print(f"ch{cid}: status={c['status']} srt={c['srt']} v={n_v} a={n_a} aac={n_aac} {'OK' if good else 'FAIL'}")
    if not good and lines:
        print("  ", " | ".join(lines[:8]))
print(f"RESULT {ok}/8 with video+4audio")
PY
