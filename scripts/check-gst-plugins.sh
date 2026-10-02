#!/usr/bin/env bash
set -euo pipefail
need=(decklinkvideosrc decklinkvideosink nvh264enc srtsink mpegtsmux mp4mux h264parse deinterlace)
miss=0
for e in "${need[@]}"; do
  if gst-inspect-1.0 "$e" >/dev/null 2>&1; then
    echo "OK  $e"
  else
    echo "MISS $e"
    miss=1
  fi
done
# alternatives
if ! gst-inspect-1.0 nvh264enc >/dev/null 2>&1; then
  if gst-inspect-1.0 nvautogpuh264enc >/dev/null 2>&1; then
    echo "OK  nvautogpuh264enc (alt)"
    miss=0
  fi
fi
exit "$miss"
