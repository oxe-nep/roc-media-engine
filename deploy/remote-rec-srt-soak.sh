#!/usr/bin/env bash
# 8ch REC/SRT regress + short soak for roc-media-engine
set -euo pipefail

ENG="${ENG:-http://127.0.0.1:8090}"
GO="${GO:-http://127.0.0.1:8080}"
REC_DIR="${REC_DIR:-/opt/applications/roc-media-engine/recordings/_unsorted}"
SOAK_SECS="${SOAK_SECS:-300}"
PASS=0
FAIL=0

log() { printf '[%s] %s\n' "$(date -u +%H:%M:%S)" "$*"; }
ok() { PASS=$((PASS+1)); log "OK  $*"; }
bad() { FAIL=$((FAIL+1)); log "FAIL $*"; }

health() {
  curl -sf "$ENG/health" | tee /tmp/me_health.json >/dev/null
  python3 - <<'PY'
import json
d=json.load(open("/tmp/me_health.json"))
assert d.get("ok") is True
assert d.get("nvenc_used",0)==8, d
print("nvenc", d["nvenc_used"], "/", d.get("nvenc_limit"))
PY
}

channels_json() {
  curl -sf "$ENG/api/channels" -o /tmp/me_channels.json
}

assert_no_errors() {
  channels_json
  python3 - <<'PY'
import json,sys
d=json.load(open("/tmp/me_channels.json"))
cs=d.get("channels", d)
errs=[]
for c in cs:
    if c.get("status")=="error" or c.get("last_error"):
        errs.append((c["id"], c.get("status"), c.get("last_error")))
if errs:
    print("errors:", errs)
    sys.exit(1)
print("channels_ok", len(cs))
PY
}

http_code() {
  local method=$1 url=$2
  curl -s -o /tmp/me_body.txt -w "%{http_code}" -X "$method" "$url"
}

mkdir -p "$REC_DIR"
log "=== baseline health ==="
health && ok "health 8/8" || bad "health"
assert_no_errors && ok "no channel errors" || bad "channel errors at start"

log "=== REC start all 8 ==="
for i in 1 2 3 4 5 6 7 8; do
  code=$(http_code POST "$ENG/api/channels/$i/record/start?label=soak")
  if [[ "$code" == "200" ]]; then ok "rec start ch$i"; else bad "rec start ch$i code=$code $(head -c 120 /tmp/me_body.txt)"; fi
done
sleep 3
channels_json
python3 - <<'PY'
import json,sys
d=json.load(open("/tmp/me_channels.json"))
cs=d.get("channels", d)
bad=[c["id"] for c in cs if not c.get("recording")]
print("recording", [c["id"] for c in cs if c.get("recording")])
if bad:
    print("not recording", bad)
    sys.exit(1)
PY
ok "all recording"

log "=== REC stop all 8 ==="
for i in 1 2 3 4 5 6 7 8; do
  code=$(http_code POST "$ENG/api/channels/$i/record/stop")
  if [[ "$code" == "200" ]]; then ok "rec stop ch$i"; else bad "rec stop ch$i code=$code $(head -c 120 /tmp/me_body.txt)"; fi
done
sleep 2
channels_json
python3 - <<'PY'
import json,sys
d=json.load(open("/tmp/me_channels.json"))
cs=d.get("channels", d)
still=[c["id"] for c in cs if c.get("recording")]
if still:
    print("still recording", still)
    sys.exit(1)
print("all stopped")
PY
ok "all rec stopped"
health && ok "health after rec cycle" || bad "health after rec"
assert_no_errors && ok "no errors after rec" || bad "errors after rec"

# Find new files (expect one per channel when labels collide would previously overwrite)
NEW_FILES=$(find "$REC_DIR" -type f -name '*.mp4' -mmin -5 2>/dev/null | wc -l | tr -d ' ')
log "new mp4 in last 5m under $REC_DIR: $NEW_FILES"
if [[ "$NEW_FILES" -ge 8 ]]; then ok "rec files written ($NEW_FILES)"; else bad "expected >=8 recent mp4, got $NEW_FILES"; fi

log "=== SRT start all 8 (config default listener ports) ==="
for i in 1 2 3 4 5 6 7 8; do
  code=$(http_code POST "$ENG/api/channels/$i/srt/start")
  if [[ "$code" == "200" ]]; then ok "srt start ch$i"; else bad "srt start ch$i code=$code $(head -c 160 /tmp/me_body.txt)"; fi
done
sleep 3
channels_json
python3 - <<'PY'
import json,sys
d=json.load(open("/tmp/me_channels.json"))
cs=d.get("channels", d)
bad=[c["id"] for c in cs if not c.get("srt")]
print("srt", [(c["id"], c.get("srt_url")) for c in cs if c.get("srt")])
if bad:
    print("not srt", bad)
    sys.exit(1)
PY
ok "all srt on"

log "=== SRT stop all 8 ==="
for i in 1 2 3 4 5 6 7 8; do
  code=$(http_code POST "$ENG/api/channels/$i/srt/stop")
  if [[ "$code" == "200" ]]; then ok "srt stop ch$i"; else bad "srt stop ch$i code=$code $(head -c 120 /tmp/me_body.txt)"; fi
done
sleep 2
channels_json
python3 - <<'PY'
import json,sys
d=json.load(open("/tmp/me_channels.json"))
cs=d.get("channels", d)
still=[c["id"] for c in cs if c.get("srt")]
if still:
    print("still srt", still)
    sys.exit(1)
print("all srt stopped")
PY
ok "all srt stopped"
health && ok "health after srt cycle" || bad "health after srt"
assert_no_errors && ok "no errors after srt" || bad "errors after srt"

log "=== Rapid REC attach/detach x3 (crash-safe) ==="
for round in 1 2 3; do
  for i in 1 2 3 4 5 6 7 8; do
    curl -sf -X POST "$ENG/api/channels/$i/record/start?label=rapid$round" >/dev/null || bad "rapid start ch$i r$round"
  done
  sleep 1
  for i in 1 2 3 4 5 6 7 8; do
    curl -sf -X POST "$ENG/api/channels/$i/record/stop" >/dev/null || bad "rapid stop ch$i r$round"
  done
  sleep 1
  health >/dev/null && ok "rapid round $round health" || bad "rapid round $round health"
done
assert_no_errors && ok "no errors after rapid" || bad "errors after rapid"

log "=== Soak ${SOAK_SECS}s with REC+SRT on ==="
for i in 1 2 3 4 5 6 7 8; do
  curl -sf -X POST "$ENG/api/channels/$i/record/start?label=soaklong" >/dev/null || bad "soak rec start ch$i"
  curl -sf -X POST "$ENG/api/channels/$i/srt/start" >/dev/null || bad "soak srt start ch$i"
done
END=$((SECONDS + SOAK_SECS))
TICK=0
while (( SECONDS < END )); do
  sleep 30
  TICK=$((TICK+1))
  if health >/dev/null && assert_no_errors >/dev/null; then
    ok "soak tick $TICK"
  else
    bad "soak tick $TICK"
    break
  fi
  # preview still serving
  code=$(curl -s -o /dev/null -w "%{http_code}" "$GO/hls/1/preview.m3u8" || echo 000)
  if [[ "$code" == "200" ]]; then ok "preview during soak"; else bad "preview $code during soak"; fi
done

log "=== teardown soak ==="
for i in 1 2 3 4 5 6 7 8; do
  curl -sf -X POST "$ENG/api/channels/$i/record/stop" >/dev/null || true
  curl -sf -X POST "$ENG/api/channels/$i/srt/stop" >/dev/null || true
done
sleep 2
health && ok "final health" || bad "final health"
assert_no_errors && ok "final no errors" || bad "final errors"

log "=== RESULT pass=$PASS fail=$FAIL ==="
if (( FAIL > 0 )); then
  exit 1
fi
exit 0
