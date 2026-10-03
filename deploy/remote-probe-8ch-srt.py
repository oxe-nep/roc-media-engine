#!/usr/bin/env python3
import json
import subprocess
import urllib.request

d = json.load(urllib.request.urlopen("http://127.0.0.1:8090/api/channels", timeout=5))
ok = 0
for c in d["channels"]:
    cid = c["id"]
    port = 9100 + cid
    p = subprocess.run(
        [
            "timeout",
            "12",
            "ffprobe",
            "-v",
            "error",
            "-analyzeduration",
            "5M",
            "-probesize",
            "5M",
            "-i",
            f"srt://127.0.0.1:{port}?mode=caller&latency=200",
            "-show_entries",
            "stream=codec_type,codec_name",
            "-of",
            "csv=p=0",
        ],
        capture_output=True,
        text=True,
    )
    types = set()
    for line in (p.stdout or "").splitlines():
        parts = [x.strip() for x in line.split(",") if x.strip()]
        if "video" in parts:
            types.add("video")
        if "audio" in parts:
            types.add("audio")
    status = "A+V" if types >= {"video", "audio"} else str(sorted(types) or ["none"])
    if types >= {"video", "audio"}:
        ok += 1
    print(
        f"ch{cid}: srt={c['srt']} br={c.get('srt_bitrate_kbps')} probe={status}"
    )
print(f"OK {ok}/8 with A+V")
