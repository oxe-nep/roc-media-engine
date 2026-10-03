#!/bin/bash
set -euo pipefail
for id in 1 2 3 4 5 6 7 8; do
  port=$((9100 + id))
  echo "=== ch${id} :${port} ==="
  out=$(timeout 15 ffprobe -v error -analyzeduration 5M -probesize 5M \
    -i "srt://127.0.0.1:${port}?mode=caller&latency=200" \
    -show_entries stream=index,codec_type,codec_name \
    -of csv=p=0 2>&1 || true)
  echo "$out" | head -20
  has_v=$(echo "$out" | grep -c ',video,' || true)
  has_a=$(echo "$out" | grep -c ',audio,' || true)
  if [[ "$has_v" -ge 1 && "$has_a" -ge 1 ]]; then
    echo "RESULT ch${id}: A+V OK"
  else
    echo "RESULT ch${id}: MISSING (v=$has_v a=$has_a)"
  fi
  echo
done
ss -ulnp | grep -E '910[1-8]' | sort || true
