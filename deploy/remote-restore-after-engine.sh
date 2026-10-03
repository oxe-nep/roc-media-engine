#!/bin/bash
set -eu
KEY=change-me
for i in 1 2 3 4 5; do
  code=$(curl -s -o /dev/null -w "%{http_code}" -X POST -H "X-API-Key: $KEY" "http://127.0.0.1:8080/api/streams/$i/start")
  echo "start $i -> $code"
done
sleep 10
for i in 1 4; do
  echo "srt $i:"
  curl -s -H "X-API-Key: $KEY" -X POST "http://127.0.0.1:8080/api/srt/$i/start"
  echo
done
sleep 4
curl -s http://127.0.0.1:8090/api/channels | python3 -m json.tool | grep -E '"id"|"status"|"srt"|srt_bitrate|encode_preset' | head -40
echo '=== mediamtx recent ==='