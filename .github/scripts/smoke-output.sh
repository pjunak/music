#!/usr/bin/env bash
set -euo pipefail
# Disposable Debian container only: real mpv processes with a null audio sink.
cd /tmp
tar -xzf /package/music-output-linux-x86_64.tar.gz
./music-output --version
./music-output --help >/dev/null
export HOME=/tmp/output-home MUSIC_STATE_DIR=/tmp/output-state
export MUSIC_SERVER_URL=http://127.0.0.1:9 MUSIC_OUTPUT_NAME=CI-speaker
export MUSIC_CONTROL_PORT=18731 MUSIC_MPV=/tmp/mpv-null
mkdir -p "$HOME" "$MUSIC_STATE_DIR"
printf 'fixture-existing-device\n' > "$MUSIC_STATE_DIR/client-id"
printf '#!/bin/sh\nexec /usr/bin/mpv --ao=null "$@"\n' > "$MUSIC_MPV"
chmod +x "$MUSIC_MPV"
pid=''
cleanup() {
  if [[ -n $pid ]]; then kill -INT "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; fi
}
trap cleanup EXIT
start() {
  ./music-output > /tmp/output.log 2>&1 &
  pid=$!
  for attempt in $(seq 1 30); do
    if curl --fail --silent http://127.0.0.1:18731/control > /tmp/control.json; then return; fi
    kill -0 "$pid" || { cat /tmp/output.log; return 1; }
    sleep 0.2
  done
  cat /tmp/output.log
  return 1
}
start
curl --fail --silent --header 'Content-Type: application/json' \
  --data '{"on":false,"volume":0.25}' http://127.0.0.1:18731/control | \
  grep -q '"volume":0.25'
[[ $(cat "$MUSIC_STATE_DIR/client-id") == fixture-existing-device ]]
kill -INT "$pid"
wait "$pid"
pid=''
start
[[ $(cat "$MUSIC_STATE_DIR/client-id") == fixture-existing-device ]]
# A supervised player failure must exit the client for systemd to restart it.
child=$(pgrep -P "$pid" -x mpv | head -n1)
[[ -n $child ]]
kill -TERM "$child"
for attempt in $(seq 1 50); do
  if ! kill -0 "$pid" 2>/dev/null; then break; fi
  sleep 0.2
done
if kill -0 "$pid" 2>/dev/null; then cat /tmp/output.log; exit 1; fi
if wait "$pid"; then echo 'Client unexpectedly succeeded after player failure' >&2; exit 1; fi
pid=''
echo 'OUTPUT_LINUX_SMOKE_OK: real mpv, local control, stable identity and child-failure exit'
