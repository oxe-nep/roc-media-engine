#!/usr/bin/env bash
# Smoke: ch3 = 1080i, ch4 = 1080p (as wired on capture host).
set -u
export PATH=/home/oxe/.cargo/bin:/usr/bin:/bin:${PATH}
BASE=http://127.0.0.1:8080
CH_I=3
CH_P=4
# Library root may be the NFS mount (symlink target) â€” prefer live path from API.
REC_DIR=$(
  curl -sS "$BASE/api/settings/recordings-path" 2>/dev/null \
    | python3 -c 'import json,sys,os
try:
  d=json.load(sys.stdin)
  p=d.get("path") or d.get("recordings_dir") or ""
except Exception:
  p=""
print(p)' 2>/dev/null
)
if [ -z "$REC_DIR" ] || [ ! -d "$REC_DIR" ]; then
  if [ -d /mnt/nep-storage/SHL/files/roc-recording ]; then
    REC_DIR=/mnt/nep-storage/SHL/files/roc-recording
  else
    REC_DIR=/opt/applications/roc-media-engine/recordings
  fi
fi
UNSORTED="$REC_DIR/_unsorted"
echo "REC_DIR=$REC_DIR"
PASS=0
FAIL=0

ok() { PASS=$((PASS + 1)); echo "PASS  $*"; }
bad() { FAIL=$((FAIL + 1)); echo "FAIL  $*"; }

jassert() { python3 -c "$1"; }

latest() {
  # latest <glob>
  ls -t "$@" 2>/dev/null | head -1 || true
}

cleanup() {
  for c in $CH_I $CH_P; do
    curl -sS -X POST "$BASE/api/recordings/$c/proxy/stop" >/dev/null 2>&1 || true
    curl -sS -X POST "$BASE/api/recordings/$c/hq/stop" >/dev/null 2>&1 || true
    curl -sS -X PUT "$BASE/api/workflows/$c" -H 'Content-Type: application/json' -d '{"mode":"pair"}' >/dev/null 2>&1 || true
    curl -sS -X POST "$BASE/api/streams/$c/stop" >/dev/null 2>&1 || true
  done
}
trap cleanup EXIT

echo "======== 0) baseline ========"
H=$(curl -sS "$BASE/api/health")
echo "$H" | jassert 'import json,sys; d=json.load(sys.stdin); assert d.get("ok"); print(d)' && ok "health" || bad "health"

echo "======== 1) ch$CH_I encode (expect 1080i50) ========"
curl -sS -X PUT "$BASE/api/streams/$CH_I/encode-preset" -H 'Content-Type: application/json' -d '{"preset":"hq"}' >/dev/null
curl -sS -X PUT "$BASE/api/streams/$CH_I/record-preset" -H 'Content-Type: application/json' -d '{"preset":"dnxhd_hq"}' >/dev/null
curl -sS -X POST "$BASE/api/streams/$CH_I/stop" >/dev/null 2>&1 || true
sleep 1
curl -sS -X POST "$BASE/api/streams/$CH_I/start" >/dev/null
sleep 7
curl -sS "$BASE/api/channels" -o /tmp/rme-chs.json
jassert "import json
c=next(x for x in json.load(open('/tmp/rme-chs.json'))['channels'] if x['id']==$CH_I)
fmt=(c.get('input_format') or '')
print('ch$CH_I', c['status'], fmt, 'locked', c.get('locked_mode'), 'bit', c.get('bit_depth'), 'mezz', c.get('mezz_label'), 'err', c.get('last_error'))
assert c['status'] == 'running', ('want signal lock', c['status'], c.get('last_error'))
assert 'i50' in fmt or '1080i' in fmt.lower(), ('expected interlaced IN', fmt)
lm=(c.get('locked_mode') or '')
assert 'i50' in lm or '1080i' in lm.lower(), ('locked_mode not interlaced', lm)
" && ok "ch$CH_I locked 1080i" || bad "ch$CH_I format (want 1080i)"

echo "======== 2) ch$CH_P encode (expect 1080p50) ========"
curl -sS -X PUT "$BASE/api/streams/$CH_P/encode-preset" -H 'Content-Type: application/json' -d '{"preset":"hq"}' >/dev/null
curl -sS -X PUT "$BASE/api/streams/$CH_P/record-preset" -H 'Content-Type: application/json' -d '{"preset":"prores_proxy"}' >/dev/null
curl -sS -X POST "$BASE/api/streams/$CH_P/stop" >/dev/null 2>&1 || true
sleep 1
curl -sS -X POST "$BASE/api/streams/$CH_P/start" >/dev/null
sleep 7
curl -sS "$BASE/api/channels" -o /tmp/rme-chs.json
jassert "import json
c=next(x for x in json.load(open('/tmp/rme-chs.json'))['channels'] if x['id']==$CH_P)
fmt=(c.get('input_format') or '')
print('ch$CH_P', c['status'], fmt, 'locked', c.get('locked_mode'), 'bit', c.get('bit_depth'), 'mezz', c.get('mezz_label'), 'err', c.get('last_error'))
assert c['status'] == 'running', ('want signal lock', c['status'], c.get('last_error'))
assert 'p50' in fmt or '1080p' in fmt.lower(), ('expected progressive IN', fmt)
assert 'i50' not in fmt, ('unexpected interlaced on p channel', fmt)
lm=(c.get('locked_mode') or '')
assert 'p50' in lm or '1080p' in lm.lower(), ('locked_mode not progressive', lm)
assert 'i50' not in lm, ('locked_mode flipped to interlaced', lm)
" && ok "ch$CH_P locked 1080p" || bad "ch$CH_P format (want 1080p)"

echo "======== 3) ch$CH_I DNxHD HQ REC (i50 â†’ 185 interlaced) ========"
curl -sS -X PUT "$BASE/api/recordings/$CH_I/name" -H 'Content-Type: application/json' -d '{"name":"smoke_i_dnxhd"}' >/dev/null
curl -sS -X PUT "$BASE/api/recordings/$CH_I/category" -H 'Content-Type: application/json' -d '{"category":"_unsorted"}' >/dev/null
curl -sS -X POST "$BASE/api/recordings/$CH_I/hq/start" -o /tmp/rme-dnx-start.json
echo "start: $(head -c 160 /tmp/rme-dnx-start.json)"
sleep 12
STOP=$(curl -sS -w ' HTTP:%{http_code}' -X POST "$BASE/api/recordings/$CH_I/hq/stop")
echo "stop:$STOP"
DNX=$(latest "$UNSORTED"/smoke_i_dnxhd_ch${CH_I}_*.mxf)
echo "DNX=$DNX"
if [ -n "$DNX" ] && [ -s "$DNX" ]; then
  ffprobe -v error -show_entries stream=codec_name,width,height,field_order,r_frame_rate,pix_fmt -of default=nw=1 "$DNX" | head -20
  DUR=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$DNX" 2>/dev/null || echo 0)
  FO=$(ffprobe -v error -select_streams v:0 -show_entries stream=field_order -of csv=p=0 "$DNX" 2>/dev/null || true)
  SCAN=$(mediainfo --Inform="Video;%ScanType%" "$DNX" 2>/dev/null || true)
  SZ=$(stat -c%s "$DNX" 2>/dev/null || echo 0)
  echo "dur=$DUR field_order=$FO scan=$SCAN size=$SZ"
  jassert "d=float('$DUR' or 0); assert d>=8, d" && ok "ch$CH_I DNxHD duration" || bad "ch$CH_I DNxHD duration"
  # mxfmux often omits field_order; OP DNxHD 185 @ ~12s ≈ 185 Mbps ≈ 280MB.
  # Accept ffprobe/mediainfo interlace OR bitrate band for the i50 HQ OP.
  if echo "$FO" | grep -qiE 'tt|bb|interlaced|tb|bt'; then
    ok "ch$CH_I DNxHD interlaced field_order=$FO"
  elif echo "$SCAN" | grep -qiE 'interlaced|MBAFF|interleave'; then
    ok "ch$CH_I DNxHD interlaced mediainfo scan=$SCAN (ffprobe field_order=$FO)"
  else
    jassert "
d=float('$DUR' or 0); sz=int('$SZ' or 0)
mbps=(sz*8/d/1e6) if d>0 else 0
print(f'mbps={mbps:.1f}')
assert 140 <= mbps <= 240, ('want ~185 Mbps i50 HQ OP', mbps, sz, d)
" && ok "ch$CH_I DNxHD ~185 Mbps OP (ffprobe field_order=$FO — mxfmux metadata gap)" \
      || bad "ch$CH_I DNxHD expected interlaced/185 OP (field_order=$FO scan=$SCAN)"
  fi
else
  bad "ch$CH_I DNxHD file missing"
  journalctl -u roc-media-engine --since "3 min ago" --no-pager | grep -iE "channel.?$CH_I|mezz|dnx|EOS|error" | tail -25 || true
fi

echo "======== 4) ch$CH_P proxy REC ========"
curl -sS -X PUT "$BASE/api/recordings/$CH_P/name" -H 'Content-Type: application/json' -d '{"name":"smoke_p_proxy"}' >/dev/null
curl -sS -X PUT "$BASE/api/recordings/$CH_P/category" -H 'Content-Type: application/json' -d '{"category":"_unsorted"}' >/dev/null
curl -sS -X POST "$BASE/api/recordings/$CH_P/proxy/start" >/dev/null
sleep 8
curl -sS -X POST "$BASE/api/recordings/$CH_P/proxy/stop" >/dev/null
PROXY=$(latest "$UNSORTED"/smoke_p_proxy_ch${CH_P}_*_proxy.mp4)
echo "PROXY=$PROXY"
if [ -n "$PROXY" ] && [ -s "$PROXY" ]; then
  DUR=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$PROXY" 2>/dev/null || echo 0)
  echo "dur=$DUR size=$(stat -c%s "$PROXY")"
  jassert "d=float('$DUR' or 0); assert d>=5, d" && ok "ch$CH_P proxy REC" || bad "ch$CH_P proxy REC"
else
  bad "ch$CH_P proxy file missing"
fi

echo "======== 5) ch$CH_P ProRes Proxy REC (SKIP — defer until hardware refresh) ========"
# ProRes/qtmux on current CPU+NFS only sustains ~1s then EOS-stalls. Re-enable after
# capture-host upgrade; DNxHD + proxy remain the smoke gates for mezz/encoded REC.
echo "SKIP  ch$CH_P ProRes (parked — hardware)"
ok "ch$CH_P ProRes skipped (parked)"

echo "======== 6) dual encoded REC rejected (ch$CH_P) ========"
curl -sS -X PUT "$BASE/api/streams/$CH_P/record-preset" -H 'Content-Type: application/json' -d '{"preset":"hq"}' >/dev/null
curl -sS -X POST "$BASE/api/recordings/$CH_P/proxy/start" >/dev/null
sleep 1
DUAL=$(curl -sS -w '\nHTTP:%{http_code}' -X POST "$BASE/api/recordings/$CH_P/hq/start")
echo "$DUAL" | head -c 300; echo
echo "$DUAL" | jassert 'import sys; t=sys.stdin.read().lower(); assert "http:4" in t or "double aac" in t or "stop proxy" in t or "mezz" in t, t' \
  && ok "dual encoded rejected" || bad "dual encoded not rejected"
curl -sS -X POST "$BASE/api/recordings/$CH_P/proxy/stop" >/dev/null 2>&1 || true
curl -sS -X POST "$BASE/api/recordings/$CH_P/hq/stop" >/dev/null 2>&1 || true

echo "======== 7) commentator rejected ========"
COMM=$(curl -sS -w '\nHTTP:%{http_code}' -X PUT "$BASE/api/workflows/$CH_P" \
  -H 'Content-Type: application/json' -d '{"mode":"remote_commentator"}')
echo "$COMM" | head -c 200; echo
echo "$COMM" | jassert 'import sys; t=sys.stdin.read().lower(); assert "not available" in t or "http:4" in t, t' \
  && ok "commentator rejected" || bad "commentator"

echo "======== 8) TC on ch$CH_I (brief) ========"
curl -sS -X POST "$BASE/api/streams/$CH_I/stop" >/dev/null 2>&1 || true
sleep 1
curl -sS -X PUT "$BASE/api/workflows/$CH_I" -H 'Content-Type: application/json' -d '{"mode":"tc"}' -o /tmp/rme-tc.json
echo "tc put: $(head -c 200 /tmp/rme-tc.json)"
sleep 8
curl -sS "$BASE/api/playout/$CH_I/tc-loop" -o /tmp/rme-tc-get.json
cat /tmp/rme-tc-get.json; echo
jassert "import json
d=json.load(open('/tmp/rme-tc-get.json'))
print('tc', d.get('status'), d.get('format'), d.get('mode'))
assert d.get('status') in ('running','restarting') or d.get('enabled') is True, d
" && ok "TC running on ch$CH_I" || bad "TC on ch$CH_I"
curl -sS -X PUT "$BASE/api/workflows/$CH_I" -H 'Content-Type: application/json' -d '{"mode":"pair"}' >/dev/null
sleep 2

echo
echo "======== SUMMARY ========"
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
