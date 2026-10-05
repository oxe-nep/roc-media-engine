#!/usr/bin/env bash
set -euo pipefail
for f in \
  "/mnt/nep-storage/Hockeyallsvenska/roc-recording/unsorted/Channel_4_2026-10-03_08-45-30_ch4.mp4" \
  "/mnt/nep-storage/Hockeyallsvenska/roc-recording/unsorted/Channel_4_2026-10-02_22-43-46_ch4.mp4" \
  "/tmp/rec_sync_test8.mp4" \
  "/tmp/rec_sync_test7.mp4"
do
  echo "==== $f ===="
  if [ ! -f "$f" ]; then echo "MISSING"; continue; fi
  ls -la "$f"
  ffprobe -hide_banner "$f" 2>&1 | head -25
  echo "--- atoms ---"
  python3 - <<PY
import struct,sys
p="$f"
with open(p,"rb") as fh:
    data=fh.read(64)
    print("head hex:", data[:32].hex())
    fh.seek(0)
    atoms=[]
    while True:
        off=fh.tell()
        hdr=fh.read(8)
        if len(hdr)<8: break
        size,typ=struct.unpack(">I4s", hdr)
        typ=typ.decode("latin1",errors="replace")
        if size==1:
            bsize=struct.unpack(">Q", fh.read(8))[0]
            atoms.append((off,typ,bsize))
            fh.seek(off+bsize)
        elif size==0:
            atoms.append((off,typ,0))
            break
        else:
            atoms.append((off,typ,size))
            fh.seek(off+size)
        if len(atoms)>20: break
print(atoms[:15])
PY
  echo
done
