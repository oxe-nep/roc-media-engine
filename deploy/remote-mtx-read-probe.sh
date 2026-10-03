#!/bin/bash
set -uo pipefail
echo "=== API/TCP ports via LB ==="
for p in 9997 8889 8888 8189 8554 8322 80 443; do
  if timeout 1 bash -c "echo >/dev/tcp/10.199.23.251/$p" 2>/dev/null; then
    echo "open:$p"
  else
    echo "closed:$p"
  fi
done

echo "=== republish ch2 via engine ==="
curl -sf -m 10 -X POST http://127.0.0.1:8090/api/channels/2/srt/stop >/dev/null || true
sleep 1
curl -sf -m 30 -X POST -H 'Content-Type: application/json' \
  -d '{"url":"srt://10.199.23.251:8890?latency=1000000&mode=caller&pkt_size=1316&streamid=publish:live/main:publisher:3fQjvsAHKXE7GAyZ"}' \
  http://127.0.0.1:8090/api/channels/2/srt/start >/dev/null
sleep 5
journalctl -u roc-media-engine --since '25 sec ago' --no-pager \
  | grep channel=2 | grep -iE 'valve|WARN|error|stereo|SRT' | tail -12

echo "=== read streamid variants ==="
while IFS= read -r sid; do
  echo "SID=$sid"
  timeout 3 ffmpeg -hide_banner -loglevel warning \
    -i "srt://10.199.23.251:8890?mode=caller&latency=1000&streamid=${sid}" \
    -t 1 -f null - 2>&1 | tail -4
  echo
done <<'EOF'
read:live/main
read:live/main:publisher:3fQjvsAHKXE7GAyZ
read:live/main:user:user
play:live/main
live/main
EOF
