#!/usr/bin/env bash
set -euo pipefail
source ~/.cargo/env
cd /opt/applications/roc-media-engine
cargo build --release -p roc-engine --features gst 2>&1 | tail -8
sudo systemctl restart roc-media-engine
sleep 2
for i in 1 2 3 4 5 6 7 8; do
  curl -s -X POST "http://127.0.0.1:8090/api/channels/$i/start" >/dev/null || true
done
sleep 12
TMP=/tmp/rec_fix_local.mp4
NFS=/mnt/nep-storage/Hockeyallsvenska/roc-recording/unsorted/rec_fix_nfs_$(date +%Y%m%d_%H%M%S).mp4
sudo rm -f "$TMP"
echo "local=$TMP"
curl -s -X POST "http://127.0.0.1:8090/api/channels/4/record/start?path=$TMP"
echo
sleep 6
curl -s -m 25 -X POST "http://127.0.0.1:8090/api/channels/4/record/stop"
echo
sleep 1
ls -la "$TMP"
ffprobe -hide_banner "$TMP" 2>&1 | head -18
echo "nfs=$NFS"
ENC=$(python3 -c "import urllib.parse; print(urllib.parse.quote('''$NFS'''))")
curl -s -X POST "http://127.0.0.1:8090/api/channels/4/record/start?path=$ENC"
echo
sleep 6
curl -s -m 25 -X POST "http://127.0.0.1:8090/api/channels/4/record/stop"
echo
sleep 1
ls -la "$NFS"
ffprobe -hide_banner "$NFS" 2>&1 | head -18
echo '=== logs ==='
journalctl -u roc-media-engine --since '2 min ago' --no-pager | grep -E 'record EOS|attached record|detached record|moov|ERROR|EOS timeout' | tail -20
