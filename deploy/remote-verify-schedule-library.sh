#!/usr/bin/env bash
# Verify library + recording schedule ticker on the capture host.
set -euo pipefail

BASE="${BASE:-http://127.0.0.1:8080}"
CH="${CH:-4}"
CAT_SRC="_unsorted"
CAT_TMP="_verify_cutover_tmp"
PASS=0
FAIL=0

ok() { echo "PASS: $*"; PASS=$((PASS + 1)); }
bad() { echo "FAIL: $*"; FAIL=$((FAIL + 1)); }

echo "=== library categories ==="
curl -sS "$BASE/api/library/categories" -o /tmp/rme-cats.json
head -c 400 /tmp/rme-cats.json; echo
python3 - <<'PY' && ok "categories include _unsorted" || bad "missing _unsorted"
import json
cats=json.load(open("/tmp/rme-cats.json"))
names=[c["name"] if isinstance(c,dict) else c for c in cats]
assert "_unsorted" in names
PY

echo "=== library files ==="
curl -sS "$BASE/api/library/files?category=$CAT_SRC" -o /tmp/rme-files.json
head -c 500 /tmp/rme-files.json; echo
NAME=$(python3 - <<'PY'
import json
raw=json.load(open("/tmp/rme-files.json"))
files=raw if isinstance(raw,list) else raw.get("files") or raw.get("items") or []
print(files[0]["name"] if files else "")
PY
)
if [ -n "$NAME" ]; then
  ok "found file $NAME"
  ENC_CAT=$(python3 -c "import urllib.parse; print(urllib.parse.quote('$CAT_SRC'))")
  ENC_NAME=$(python3 -c "import urllib.parse; print(urllib.parse.quote('''$NAME'''))")
  CODE=$(curl -sS -o /tmp/rme-lib-sample.bin -w "%{http_code}" \
    "$BASE/api/library/file/$ENC_CAT/$ENC_NAME")
  SIZE=$(wc -c </tmp/rme-lib-sample.bin)
  if [ "$CODE" = "200" ] && [ "$SIZE" -gt 1000 ]; then
    ok "file serve HTTP $CODE size=$SIZE"
  else
    bad "file serve HTTP $CODE size=$SIZE"
  fi
else
  bad "no files in $CAT_SRC to serve"
fi

echo "=== library move (round-trip) ==="
if [ -n "${NAME:-}" ]; then
  curl -sS -X POST "$BASE/api/library/categories" \
    -H 'Content-Type: application/json' \
    -d "{\"name\":\"$CAT_TMP\"}" -o /tmp/rme-mkcat.json || true
  curl -sS -X POST "$BASE/api/library/move" \
    -H 'Content-Type: application/json' \
    -d "{\"from_category\":\"$CAT_SRC\",\"to_category\":\"$CAT_TMP\",\"name\":$(python3 -c "import json; print(json.dumps('''$NAME'''))")}" \
    -o /tmp/rme-move.json
  head -c 300 /tmp/rme-move.json; echo
  if python3 -c "import json; d=json.load(open('/tmp/rme-move.json')); assert 'error' not in d"; then
    ok "moved to $CAT_TMP"
  else
    bad "move to tmp failed"
  fi
  curl -sS -X POST "$BASE/api/library/move" \
    -H 'Content-Type: application/json' \
    -d "{\"from_category\":\"$CAT_TMP\",\"to_category\":\"$CAT_SRC\",\"name\":$(python3 -c "import json; print(json.dumps('''$NAME'''))")}" \
    -o /tmp/rme-move2.json
  if python3 -c "import json; d=json.load(open('/tmp/rme-move2.json')); assert 'error' not in d"; then
    ok "moved back to $CAT_SRC"
  else
    bad "move back failed"
  fi
  curl -sS -X DELETE "$BASE/api/library/categories/$CAT_TMP" -o /tmp/rme-delcat.json || true
else
  bad "skip move — no sample file"
fi

echo "=== schedule on channel $CH ==="
curl -sS -X POST "$BASE/api/streams/$CH/start" -o /tmp/rme-start.json || true
sleep 4
curl -sS "$BASE/api/channels" -o /tmp/rme-chs.json
CAP=$(python3 - <<PY
import json
chs=json.load(open("/tmp/rme-chs.json"))["channels"]
ch=next(c for c in chs if c["id"]==$CH)
print(ch.get("status"))
print("recording=", ch.get("recording"), file=__import__("sys").stderr)
PY
)
echo "channel status: $CAP"
if [ "$CAP" != "running" ] && [ "$CAP" != "waiting" ]; then
  bad "capture not running on ch$CH (status=$CAP) — cannot verify schedule"
else
  ok "capture $CAP on ch$CH"
  START=$(date -u -d '+15 seconds' +%Y-%m-%dT%H:%M:%SZ)
  STOP=$(date -u -d '+45 seconds' +%Y-%m-%dT%H:%M:%SZ)
  echo "schedule $START -> $STOP"
  curl -sS -X PUT "$BASE/api/recordings/$CH/category" \
    -H 'Content-Type: application/json' \
    -d '{"category":"_unsorted"}' >/dev/null
  curl -sS -X PUT "$BASE/api/recordings/$CH/name" \
    -H 'Content-Type: application/json' \
    -d '{"name":"verify_sched"}' >/dev/null
  curl -sS -X PUT "$BASE/api/recordings/$CH/schedule" \
    -H 'Content-Type: application/json' \
    -d "{\"start_at\":\"$START\",\"stop_at\":\"$STOP\"}" -o /tmp/rme-sched.json
  head -c 400 /tmp/rme-sched.json; echo
  python3 -c "import json; d=json.load(open('/tmp/rme-sched.json')); assert 'schedule' in d or 'start_at' in str(d)" \
    && ok "schedule set" || bad "schedule set response unexpected"

  echo "waiting for schedule start..."
  STARTED=0
  for i in $(seq 1 40); do
    sleep 1
    REC=$(curl -sS "$BASE/api/channels" | python3 -c "import json,sys; chs=json.load(sys.stdin)['channels']; c=next(x for x in chs if x['id']==$CH); print('1' if c.get('recording') else '0')")
    if [ "$REC" = "1" ]; then STARTED=1; echo "recording active at +${i}s"; break; fi
  done
  if [ "$STARTED" = "1" ]; then ok "schedule started recording"; else bad "schedule did not start recording"; fi

  if [ "$STARTED" = "1" ]; then
    echo "waiting for schedule stop..."
    STOPPED=0
    for i in $(seq 1 50); do
      sleep 1
      REC=$(curl -sS "$BASE/api/channels" | python3 -c "import json,sys; chs=json.load(sys.stdin)['channels']; c=next(x for x in chs if x['id']==$CH); print('1' if c.get('recording') else '0')")
      if [ "$REC" = "0" ]; then STOPPED=1; echo "recording stopped at +${i}s after start"; break; fi
    done
    if [ "$STOPPED" = "1" ]; then ok "schedule stopped recording"; else bad "schedule did not stop recording"; fi
    curl -sS -X DELETE "$BASE/api/recordings/$CH/schedule" >/dev/null || true
  fi
fi

echo
echo "=== summary PASS=$PASS FAIL=$FAIL ==="
[ "$FAIL" -eq 0 ]
