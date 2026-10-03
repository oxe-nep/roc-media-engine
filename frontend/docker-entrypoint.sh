#!/bin/sh
set -e

: "${BACKEND_HOST:=10.199.28.249}"
: "${BACKEND_PORT:=8080}"
: "${API_KEY:=change-me}"

CONF_DIR=/etc/nginx/http.d
if [ ! -d "$CONF_DIR" ]; then
  CONF_DIR=/etc/nginx/conf.d
  mkdir -p "$CONF_DIR"
fi

envsubst '${BACKEND_HOST} ${BACKEND_PORT} ${API_KEY}' \
  < /etc/nginx/templates/default.conf.template \
  > "$CONF_DIR/default.conf"

# Next.js standalone server
HOSTNAME=0.0.0.0 PORT=3000 node server.js &

exec nginx -g 'daemon off;'
