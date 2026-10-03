#!/bin/bash
set -euo pipefail
for id in 1 2 3 4 5 6 7 8; do
  curl -sf -m 45 -X POST "http://127.0.0.1:8080/api/streams/${id}/start" >/dev/null || true
done
sleep 8
curl -sf http://127.0.0.1:8080/api/streams | python3 -c 'import sys,json; d=json.load(sys.stdin); print([(x["id"], x["status"]) for x in d])'
curl -sf http://127.0.0.1:8080/api/health; echo
curl -sf -o /dev/null -w "hls %{http_code}\n" http://127.0.0.1:8080/hls/1/preview.m3u8 || true
