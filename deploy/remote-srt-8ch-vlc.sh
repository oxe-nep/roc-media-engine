#!/bin/bash
# Start SRT listeners on all running channels (VLC: srt://HOST:910N?mode=caller&latency=120)
set -euo pipefail
HOST="${1:-}"
if [[ -z "$HOST" ]]; then
  HOST=$(hostname -I | awk '{print $1}')
fi

echo "=== channel status ==="
python3 - <<'PY'
import json, urllib.request
d = json.load(urllib.request.urlopen("http://127.0.0.1:8090/api/channels", timeout=5))
for c in d["channels"]:
    print(
        f"ch{c['id']}: status={c['status']} srt={c['srt']} "
        f"vbr={c.get('video_bitrate_kbps')} srt_br={c.get('srt_bitrate_kbps')} "
        f"url={c.get('srt_url')}"
    )
PY

echo
echo "=== ensure capture + SRT listener on ch1-8 ==="
for id in 1 2 3 4 5 6 7 8; do
  # Start capture if needed (ignore failures for missing signal)
  curl -sf -m 45 -X POST "http://127.0.0.1:8090/api/channels/${id}/start" >/dev/null 2>&1 || true
done
sleep 8

for id in 1 2 3 4 5 6 7 8; do
  st=$(python3 - <<PY
import json, urllib.request
d=json.load(urllib.request.urlopen("http://127.0.0.1:8090/api/channels", timeout=5))
c=next(x for x in d["channels"] if x["id"]==$id)
print(c["status"])
PY
)
  if [[ "$st" != "running" ]]; then
    echo "ch${id}: skip SRT (status=$st)"
    continue
  fi
  curl -sf -m 10 -X POST "http://127.0.0.1:8090/api/channels/${id}/srt/stop" >/dev/null 2>&1 || true
  sleep 1
  port=$((9100 + id))
  url="srt://0.0.0.0:${port}?mode=listener&latency=120"
  echo -n "ch${id}: SRT listener :${port} … "
  if curl -sf -m 30 -X POST -H 'Content-Type: application/json' \
      -d "{\"url\":\"${url}\"}" \
      "http://127.0.0.1:8090/api/channels/${id}/srt/start" >/dev/null; then
    echo OK
  else
    echo FAIL
  fi
done

sleep 6
echo
echo "=== listeners / bitrate ==="
python3 - <<PY
import json, urllib.request
d = json.load(urllib.request.urlopen("http://127.0.0.1:8090/api/channels", timeout=5))
host = "$HOST"
for c in d["channels"]:
    port = 9100 + c["id"]
    vlc = f"srt://{host}:{port}?mode=caller&latency=120"
    print(
        f"ch{c['id']}: status={c['status']} srt={c['srt']} "
        f"srt_br={c.get('srt_bitrate_kbps')}  VLC={vlc}"
    )
PY

echo
echo "=== probe A/V on first running SRT channel (2s) ==="
first=""
for id in 1 2 3 4 5 6 7 8; do
  srt=$(python3 - <<PY
import json, urllib.request
d=json.load(urllib.request.urlopen("http://127.0.0.1:8090/api/channels", timeout=5))
c=next(x for x in d["channels"] if x["id"]==$id)
print("1" if c.get("srt") else "0")
PY
)
  if [[ "$srt" == "1" ]]; then
    first=$id
    break
  fi
done
if [[ -n "$first" ]]; then
  port=$((9100 + first))
  timeout 8 ffprobe -v error -hide_banner \
    -i "srt://127.0.0.1:${port}?mode=caller&latency=120" \
    -show_entries stream=index,codec_type,codec_name \
    -of csv=p=0 2>&1 | head -20 || true
else
  echo "no SRT channel to probe"
fi

echo
journalctl -u roc-media-engine --since '90 sec ago' --no-pager \
  | grep -iE 'SRT A/V input|SRT valve open|SRT baked|timeout|ERROR' \
  | tail -30
