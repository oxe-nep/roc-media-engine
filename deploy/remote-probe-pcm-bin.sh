#!/usr/bin/env bash
set -eu
python3 <<'PY'
pair=0
rows=[]
for out_ch in range(2):
  coeffs=[]
  for in_ch in range(8):
    coeffs.append('1.0' if in_ch==pair*2+out_ch else '0.0')
  rows.append('<'+', '.join(coeffs)+'>')
matrix='<'+', '.join(rows)+'>'
print('matrix', repr(matrix))
desc=f'audioconvert mix-matrix="{matrix}" ! audio/x-raw,format=S24LE,channels=2,rate=48000,layout=interleaved'
print('desc', desc)
import gi
gi.require_version('Gst','1.0')
from gi.repository import Gst
Gst.init(None)
for name, d in [('s24', desc), ('aac', f'audioconvert mix-matrix="{matrix}" ! audio/x-raw,channels=2 ! voaacenc bitrate=192000 ! aacparse')]:
  try:
    b=Gst.parse_bin_from_description(d, True)
    print(name, 'OK', b)
  except Exception as e:
    print(name, 'FAIL', e)
PY

echo "=== find mxfmux assert ==="
find /usr -name 'mxfmux.c' 2>/dev/null | head -3 || true
# apt source if available
sed -n '1370,1400p' /usr/share/doc/gstreamer1.0-plugins-bad/examples/*/mxfmux.c 2>/dev/null || true
