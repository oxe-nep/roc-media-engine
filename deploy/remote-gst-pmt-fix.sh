#!/bin/bash
set -euo pipefail
# shellcheck disable=SC1090
source "$HOME/.cargo/env"
cd /opt/applications/roc-media-engine
cargo test -p roc-pipelines --features gst pmt_requires_both -- --nocapture
cargo build --release -p roc-engine --features gst
sudo systemctl restart roc-media-engine
sleep 4
systemctl is-active roc-media-engine

# Ensure ch4 capture + SRT via engine (8090)
curl -sf -m 60 -X POST "http://127.0.0.1:8090/api/channels/4/start" || true
sleep 6
curl -sf -m 10 -X POST "http://127.0.0.1:8090/api/channels/4/srt/stop" || true
sleep 2
curl -sf -m 30 -X POST -H 'Content-Type: application/json' \
  -d '{"url":"srt://10.199.23.251:8890?latency=1000000&mode=caller&pkt_size=1316&streamid=publish:live/main:publisher:3fQjvsAHKXE7GAyZ"}' \
  "http://127.0.0.1:8090/api/channels/4/srt/start"
echo
sleep 10
journalctl -u roc-media-engine --since '60 sec ago' --no-pager \
  | grep -iE 'SRT valve|full A/V|waiting for full|audio valve|timeout|baked|channel=4' \
  | tail -40
python3 - <<'PY'
import json,urllib.request
d=json.load(urllib.request.urlopen('http://127.0.0.1:8090/api/channels', timeout=5))
for c in d['channels']:
    if c['id']==4:
        print('ch4', c['status'], c['srt'], c.get('srt_bitrate_kbps'), (c.get('srt_url') or '')[:90])
try:
    d=json.load(urllib.request.urlopen('http://10.199.23.251:9997/v3/paths/list', timeout=5))
    for i in d.get('items',[]):
        print('mtx', i['name'], 'ready=', i.get('ready'), 'tracks=', i.get('tracks'))
except Exception as e:
    print('mtx err', e)
PY
