#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <image>" >&2
  exit 2
fi

readonly image="$1"

docker image inspect "$image" >/dev/null

readonly configured_user="$(docker image inspect --format '{{.Config.User}}' "$image")"
if [[ "$configured_user" != "music" ]]; then
  echo "Rust image must configure the unprivileged music user; found '$configured_user'" >&2
  exit 1
fi

docker run --rm --entrypoint sh "$image" -ceu '
  for command_name in python python3 pip pip3 uv cargo rustc rustup node npm; do
    if command -v "$command_name" >/dev/null 2>&1; then
      echo "unexpected build/runtime command in Rust image: $command_name" >&2
      exit 1
    fi
  done

  for required_path in \
    /app/static/index.html \
    /seeds/modes \
    /usr/local/bin/music-server \
    /usr/local/bin/music-cli \
    /usr/share/doc/music/THIRD_PARTY_NOTICES.md; do
    if [ ! -r "$required_path" ]; then
      echo "required Rust image artifact is not readable: $required_path" >&2
      exit 1
    fi
  done

  if [ ! -x /usr/local/bin/music-server ] || [ ! -x /usr/local/bin/music-cli ]; then
    echo "Rust server and CLI must both be executable" >&2
    exit 1
  fi
  if [ ! -w /data ]; then
    echo "/data must be writable by the runtime user" >&2
    exit 1
  fi
  if [ -w /app ] || [ -w /seeds ]; then
    echo "/app and /seeds must remain immutable to the runtime user" >&2
    exit 1
  fi
'

docker run --rm --user 0 --entrypoint sh "$image" -ceu '
  unexpected_python="$(find / -xdev \( -type f -o -type l \) \
    \( -name "*.py" -o -name "*.pyc" -o -name "*.pyo" -o -name "python[0-9]*" \
       -o -name "libpython*" \) -print -quit)"
  if [ -n "$unexpected_python" ]; then
    echo "Python artifact remains in Rust image: $unexpected_python" >&2
    exit 1
  fi

  unexpected_application_artifact="$(find /app /data /seeds /usr/share/doc/music -xdev \
    \( -name .git -o -name .env -o -name ".env.*" -o -name __pycache__ \
       -o -name backend -o -name node_modules -o -name target \
       -o -name Cargo.toml -o -name Cargo.lock -o -name package.json \
       -o -name package-lock.json -o -name "*.db" -o -name "*.sqlite" \
       -o -name "*.sqlite3" -o -name "*.key" -o -name "*.p12" -o -name "*.pfx" \
       -o -name "*.aac" -o -name "*.flac" -o -name "*.m4a" -o -name "*.mp3" \
       -o -name "*.ogg" -o -name "*.opus" -o -name "*.wav" -o -name "*.wma" \) \
    -print -quit)"
  if [ -n "$unexpected_application_artifact" ]; then
    echo "generated, sensitive, or build artifact remains in Rust image: $unexpected_application_artifact" >&2
    exit 1
  fi

  unexpected_data="$(find /data -mindepth 1 -print -quit)"
  if [ -n "$unexpected_data" ]; then
    echo "Rust release image must not contain baked application data: $unexpected_data" >&2
    exit 1
  fi
'

echo "Rust image boundary verified: non-root, immutable application, empty data, no build or Python runtime"
