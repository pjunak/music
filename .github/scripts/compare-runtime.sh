#!/usr/bin/env bash
set -Eeuo pipefail

if (( $# < 2 || $# > 3 )); then
  echo "usage: $0 <python-image> <rust-image> [report-path]" >&2
  exit 2
fi

readonly python_image="$1"
readonly rust_image="$2"
readonly report_path="${3:-runtime-performance.md}"
readonly http_iterations="${PERF_HTTP_ITERATIONS:-200}"
readonly range_iterations="${PERF_RANGE_ITERATIONS:-100}"
readonly upload_iterations="${PERF_UPLOAD_ITERATIONS:-12}"
readonly ws_iterations="${PERF_WS_ITERATIONS:-40}"
readonly track_count="${PERF_TRACK_COUNT:-64}"
readonly startup_timeout_seconds="${PERF_STARTUP_TIMEOUT_SECONDS:-120}"
readonly python_port="${PERF_PYTHON_PORT:-18001}"
readonly rust_port="${PERF_RUST_PORT:-18002}"
readonly benchmark_username="performance"
readonly benchmark_password="synthetic-benchmark-password"
readonly memory_ceiling_bytes=3650722201
readonly run_suffix="${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-0}-$$"
readonly work_dir="$(mktemp -d)"
readonly fixture_path="$work_dir/reference.flac"
readonly ws_probe="$(cd "$(dirname "$0")" && pwd)/ws-startup-latency.mjs"

declare -a containers=()
declare -a volumes=()
declare -a monitor_pids=()
declare -a failures=()
declare -A metric=()

cleanup() {
  local pid container volume
  for pid in "${monitor_pids[@]}"; do
    kill "$pid" >/dev/null 2>&1 || true
  done
  for container in "${containers[@]}"; do
    docker rm --force "$container" >/dev/null 2>&1 || true
  done
  for volume in "${volumes[@]}"; do
    docker volume rm --force "$volume" >/dev/null 2>&1 || true
  done
  rm -rf "$work_dir"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

for command in docker curl ffmpeg node awk sort sed date; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "required command is unavailable: $command" >&2
    exit 2
  fi
done

for value in \
  "$http_iterations" "$range_iterations" "$upload_iterations" \
  "$ws_iterations" "$track_count" "$startup_timeout_seconds"; do
  if ! [[ "$value" =~ ^[1-9][0-9]*$ ]]; then
    echo "benchmark counts and timeout must be positive integers" >&2
    exit 2
  fi
done

readonly python_volume="music-perf-python-$run_suffix"
readonly rust_volume="music-perf-rust-$run_suffix"

ffmpeg -hide_banner -loglevel error \
  -f lavfi -i "sine=frequency=440:sample_rate=48000" \
  -t 12 -c:a flac "$fixture_path"

prepare_volume() {
  local volume="$1"
  docker volume create "$volume" >/dev/null
  volumes+=("$volume")
  docker run --rm \
    --user 0 \
    --volume "$volume:/data" \
    --volume "$work_dir:/fixture:ro" \
    --env PERF_TRACK_COUNT="$track_count" \
    --entrypoint sh \
    "$python_image" \
    -c '
      set -eu
      mkdir -p /data/music /data/sfx /data/modes
      index=1
      while [ "$index" -le "$PERF_TRACK_COUNT" ]; do
        padded=$(printf "%03d" "$index")
        cp /fixture/reference.flac "/data/music/track-${padded}.flac"
        index=$((index + 1))
      done
      chown -R 1000:1000 /data
    '
}

STARTED_CONTAINER=""
STARTUP_MS=""

start_candidate() {
  local label="$1"
  local phase="$2"
  local image="$3"
  local volume="$4"
  local port="$5"
  local readiness_path="$6"
  local name="music-perf-${label}-${phase}-$run_suffix"
  local started_ns deadline status

  started_ns=$(date +%s%N)
  STARTED_CONTAINER=$(docker run --detach \
    --name "$name" \
    --cpus 3 \
    --memory 4g \
    --pids-limit 512 \
    --publish "127.0.0.1:${port}:8000" \
    --volume "$volume:/data" \
    --env SESSION_COOKIE_SECURE=false \
    --env LOG_LEVEL=warn \
    "$image")
  containers+=("$STARTED_CONTAINER")

  deadline=$((SECONDS + startup_timeout_seconds))
  while true; do
    status=$(curl --silent --output /dev/null --write-out '%{http_code}' \
      "http://127.0.0.1:${port}${readiness_path}" || true)
    if [[ "$status" == "200" ]]; then
      break
    fi
    if (( SECONDS >= deadline )); then
      echo "$label $phase startup timed out; container logs follow" >&2
      docker logs "$STARTED_CONTAINER" >&2 || true
      return 1
    fi
    sleep 0.2
  done
  STARTUP_MS=$(( ($(date +%s%N) - started_ns) / 1000000 ))
}

stop_candidate() {
  local container="$1"
  docker stop --time 10 "$container" >/dev/null
  docker rm "$container" >/dev/null
}

container_memory_bytes() {
  local container="$1"
  docker exec "$container" sh -c '
    if [ -r /sys/fs/cgroup/memory.current ]; then
      cat /sys/fs/cgroup/memory.current
    else
      awk "/VmRSS:/ { print \$2 * 1024; exit }" /proc/1/status
    fi
  '
}

monitor_memory() {
  local container="$1"
  local output="$2"
  local running bytes
  while true; do
    running=$(docker inspect --format '{{.State.Running}}' "$container" 2>/dev/null || true)
    [[ "$running" == "true" ]] || break
    bytes=$(container_memory_bytes "$container" 2>/dev/null || true)
    if [[ "$bytes" =~ ^[0-9]+$ ]]; then
      printf '%s\n' "$bytes" >>"$output"
    fi
    sleep 0.2
  done
}

warm_get() {
  local url="$1"
  local attempt
  for ((attempt = 0; attempt < 10; attempt += 1)); do
    curl --fail --silent --show-error --output /dev/null "$url"
  done
}

bench_get() {
  local url="$1"
  local iterations="$2"
  local output="$3"
  local attempt elapsed
  : >"$output"
  for ((attempt = 0; attempt < iterations; attempt += 1)); do
    elapsed=$(curl --fail --silent --show-error --output /dev/null \
      --write-out '%{time_total}' "$url")
    awk -v seconds="$elapsed" 'BEGIN { printf "%.3f\n", seconds * 1000 }' >>"$output"
  done
}

bench_range() {
  local url="$1"
  local iterations="$2"
  local output="$3"
  local attempt result status elapsed
  : >"$output"
  for ((attempt = 0; attempt < iterations; attempt += 1)); do
    result=$(curl --silent --show-error --output /dev/null \
      --header 'Range: bytes=0-65535' \
      --write-out '%{http_code} %{time_total}' "$url")
    read -r status elapsed <<<"$result"
    if [[ "$status" != "206" ]]; then
      echo "range request returned HTTP $status from $url" >&2
      return 1
    fi
    awk -v seconds="$elapsed" 'BEGIN { printf "%.3f\n", seconds * 1000 }' >>"$output"
  done
}

login() {
  local base_url="$1"
  local cookie_jar="$2"
  curl --fail --silent --show-error \
    --cookie-jar "$cookie_jar" \
    --header 'Content-Type: application/json' \
    --data "{\"username\":\"$benchmark_username\",\"password\":\"$benchmark_password\"}" \
    --output /dev/null \
    "$base_url/api/auth/login"
}

bench_upload() {
  local base_url="$1"
  local cookie_jar="$2"
  local iterations="$3"
  local output="$4"
  local attempt result status elapsed
  : >"$output"
  for ((attempt = 0; attempt < iterations; attempt += 1)); do
    result=$(curl --silent --show-error --output /dev/null \
      --cookie "$cookie_jar" \
      --form "files=@${fixture_path};filename=benchmark-upload.flac;type=audio/flac" \
      --write-out '%{http_code} %{time_total}' \
      "$base_url/api/library/upload?dest=Performance&conflict=overwrite")
    read -r status elapsed <<<"$result"
    if [[ "$status" != "201" ]]; then
      echo "upload returned HTTP $status from $base_url" >&2
      return 1
    fi
    awk -v seconds="$elapsed" 'BEGIN { printf "%.3f\n", seconds * 1000 }' >>"$output"
  done
}

percentile() {
  local input="$1"
  local percentage="$2"
  local sorted="$input.sorted"
  local count line
  sort -n "$input" >"$sorted"
  count=$(awk 'END { print NR }' "$sorted")
  line=$(( (count * percentage + 99) / 100 ))
  sed -n "${line}p" "$sorted"
}

create_benchmark_users() {
  docker run --rm \
    --volume "$python_volume:/data" \
    --entrypoint python \
    "$python_image" \
    -m app.cli create-user "$benchmark_username" --password "$benchmark_password" \
    >/dev/null

  printf '%s\n' "$benchmark_password" | docker run --rm --interactive \
    --volume "$rust_volume:/data" \
    --entrypoint music-cli \
    "$rust_image" \
    create-user "$benchmark_username" --password-stdin \
    >/dev/null
}

benchmark_candidate() {
  local label="$1"
  local image="$2"
  local volume="$3"
  local port="$4"
  local readiness_path="$5"
  local base_url="http://127.0.0.1:$port"
  local ws_url="ws://127.0.0.1:$port/api/ws"
  local cookie_jar="$work_dir/$label.cookies"
  local memory_samples="$work_dir/$label.memory"
  local idle_memory monitor_pid

  start_candidate "$label" warm "$image" "$volume" "$port" "$readiness_path"
  local container="$STARTED_CONTAINER"
  metric["$label.warm_start_ms"]="$STARTUP_MS"

  curl --fail --silent --show-error --output /dev/null "$base_url/api/library/tracks/1"
  login "$base_url" "$cookie_jar"
  warm_get "$base_url/api/health"
  warm_get "$base_url/api/library/tracks/1"
  sleep 2
  idle_memory=$(container_memory_bytes "$container")
  metric["$label.idle_memory_bytes"]="$idle_memory"
  printf '%s\n' "$idle_memory" >"$memory_samples"

  monitor_memory "$container" "$memory_samples" &
  monitor_pid=$!
  monitor_pids+=("$monitor_pid")

  bench_get "$base_url/api/health" "$http_iterations" "$work_dir/$label.health"
  bench_get "$base_url/api/library/tracks/1" "$http_iterations" "$work_dir/$label.metadata"
  bench_range "$base_url/api/library/tracks/1/stream" "$range_iterations" \
    "$work_dir/$label.range"
  bench_upload "$base_url" "$cookie_jar" "$upload_iterations" "$work_dir/$label.upload"
  node "$ws_probe" "$ws_url" "$ws_iterations" 5 >"$work_dir/$label.ws"

  kill "$monitor_pid" >/dev/null 2>&1 || true
  wait "$monitor_pid" >/dev/null 2>&1 || true
  metric["$label.peak_memory_bytes"]="$(
    awk 'max < $1 { max = $1 } END { print max + 0 }' "$memory_samples"
  )"
  metric["$label.health_p50_ms"]="$(percentile "$work_dir/$label.health" 50)"
  metric["$label.health_p95_ms"]="$(percentile "$work_dir/$label.health" 95)"
  metric["$label.metadata_p50_ms"]="$(percentile "$work_dir/$label.metadata" 50)"
  metric["$label.metadata_p95_ms"]="$(percentile "$work_dir/$label.metadata" 95)"
  metric["$label.range_p50_ms"]="$(percentile "$work_dir/$label.range" 50)"
  metric["$label.range_p95_ms"]="$(percentile "$work_dir/$label.range" 95)"
  metric["$label.upload_p50_ms"]="$(percentile "$work_dir/$label.upload" 50)"
  metric["$label.upload_p95_ms"]="$(percentile "$work_dir/$label.upload" 95)"
  metric["$label.ws_p50_ms"]="$(percentile "$work_dir/$label.ws" 50)"
  metric["$label.ws_p95_ms"]="$(percentile "$work_dir/$label.ws" 95)"

  stop_candidate "$container"
}

delta_percent() {
  awk -v baseline="$1" -v candidate="$2" '
    BEGIN {
      if (baseline == 0) { print "n/a"; exit }
      printf "%+.1f%%", ((candidate - baseline) / baseline) * 100
    }
  '
}

mebibytes() {
  awk -v bytes="$1" 'BEGIN { printf "%.1f MiB", bytes / 1048576 }'
}

check_latency() {
  local name="$1"
  local baseline="$2"
  local candidate="$3"
  local absolute_floor_ms="$4"
  if ! awk -v baseline="$baseline" -v candidate="$candidate" -v floor="$absolute_floor_ms" '
    BEGIN { exit !(candidate <= baseline * 1.25 + floor) }
  '; then
    failures+=("$name exceeded Python p95 by more than 25% plus ${absolute_floor_ms} ms")
  fi
}

prepare_volume "$python_volume"
prepare_volume "$rust_volume"

start_candidate python cold "$python_image" "$python_volume" "$python_port" "/api/health"
metric[python.cold_start_ms]="$STARTUP_MS"
stop_candidate "$STARTED_CONTAINER"

start_candidate rust cold "$rust_image" "$rust_volume" "$rust_port" "/api/readiness"
metric[rust.cold_start_ms]="$STARTUP_MS"
stop_candidate "$STARTED_CONTAINER"

create_benchmark_users

benchmark_candidate python "$python_image" "$python_volume" "$python_port" "/api/health"
benchmark_candidate rust "$rust_image" "$rust_volume" "$rust_port" "/api/readiness"

metric[python.image_bytes]="$(docker image inspect --format '{{.Size}}' "$python_image")"
metric[rust.image_bytes]="$(docker image inspect --format '{{.Size}}' "$rust_image")"

check_latency "cold startup" "${metric[python.cold_start_ms]}" "${metric[rust.cold_start_ms]}" 2000
check_latency "warm startup" "${metric[python.warm_start_ms]}" "${metric[rust.warm_start_ms]}" 2000
check_latency "health API" "${metric[python.health_p95_ms]}" "${metric[rust.health_p95_ms]}" 2
check_latency "track metadata API" "${metric[python.metadata_p95_ms]}" \
  "${metric[rust.metadata_p95_ms]}" 3
check_latency "range streaming" "${metric[python.range_p95_ms]}" \
  "${metric[rust.range_p95_ms]}" 5
check_latency "upload" "${metric[python.upload_p95_ms]}" "${metric[rust.upload_p95_ms]}" 25
check_latency "WebSocket connection-to-state" "${metric[python.ws_p95_ms]}" \
  "${metric[rust.ws_p95_ms]}" 10

if (( metric[rust.peak_memory_bytes] > memory_ceiling_bytes )); then
  failures+=("Rust peak container memory exceeded the 15%-headroom ceiling under the synthetic load")
fi

mkdir -p "$(dirname "$report_path")"
status="PASS"
if (( ${#failures[@]} > 0 )); then
  status="FAIL"
fi

cat >"$report_path" <<EOF
# Python to Rust runtime comparison

- Status: **$status**
- Revision: \`${GITHUB_SHA:-local-worktree}\`
- Generated: $(date -u +'%Y-%m-%dT%H:%M:%SZ')
- Limits: 3 CPUs, 4 GiB, 512 PIDs per application container
- Synthetic corpus: $track_count copies of one generated 12-second FLAC; no private media
- Samples: HTTP $http_iterations, range $range_iterations, upload $upload_iterations, WebSocket $ws_iterations

| Metric | Python | Rust | Rust delta |
|---|---:|---:|---:|
| Cold startup + initial scan | ${metric[python.cold_start_ms]} ms | ${metric[rust.cold_start_ms]} ms | $(delta_percent "${metric[python.cold_start_ms]}" "${metric[rust.cold_start_ms]}") |
| Warm startup + reconciliation | ${metric[python.warm_start_ms]} ms | ${metric[rust.warm_start_ms]} ms | $(delta_percent "${metric[python.warm_start_ms]}" "${metric[rust.warm_start_ms]}") |
| Health API p50 | ${metric[python.health_p50_ms]} ms | ${metric[rust.health_p50_ms]} ms | $(delta_percent "${metric[python.health_p50_ms]}" "${metric[rust.health_p50_ms]}") |
| Health API p95 | ${metric[python.health_p95_ms]} ms | ${metric[rust.health_p95_ms]} ms | $(delta_percent "${metric[python.health_p95_ms]}" "${metric[rust.health_p95_ms]}") |
| Track metadata p50 | ${metric[python.metadata_p50_ms]} ms | ${metric[rust.metadata_p50_ms]} ms | $(delta_percent "${metric[python.metadata_p50_ms]}" "${metric[rust.metadata_p50_ms]}") |
| Track metadata p95 | ${metric[python.metadata_p95_ms]} ms | ${metric[rust.metadata_p95_ms]} ms | $(delta_percent "${metric[python.metadata_p95_ms]}" "${metric[rust.metadata_p95_ms]}") |
| 64 KiB range stream p50 | ${metric[python.range_p50_ms]} ms | ${metric[rust.range_p50_ms]} ms | $(delta_percent "${metric[python.range_p50_ms]}" "${metric[rust.range_p50_ms]}") |
| 64 KiB range stream p95 | ${metric[python.range_p95_ms]} ms | ${metric[rust.range_p95_ms]} ms | $(delta_percent "${metric[python.range_p95_ms]}" "${metric[rust.range_p95_ms]}") |
| Authenticated overwrite upload p50 | ${metric[python.upload_p50_ms]} ms | ${metric[rust.upload_p50_ms]} ms | $(delta_percent "${metric[python.upload_p50_ms]}" "${metric[rust.upload_p50_ms]}") |
| Authenticated overwrite upload p95 | ${metric[python.upload_p95_ms]} ms | ${metric[rust.upload_p95_ms]} ms | $(delta_percent "${metric[python.upload_p95_ms]}" "${metric[rust.upload_p95_ms]}") |
| WebSocket connect-to-state p50 | ${metric[python.ws_p50_ms]} ms | ${metric[rust.ws_p50_ms]} ms | $(delta_percent "${metric[python.ws_p50_ms]}" "${metric[rust.ws_p50_ms]}") |
| WebSocket connect-to-state p95 | ${metric[python.ws_p95_ms]} ms | ${metric[rust.ws_p95_ms]} ms | $(delta_percent "${metric[python.ws_p95_ms]}" "${metric[rust.ws_p95_ms]}") |
| Idle container memory | $(mebibytes "${metric[python.idle_memory_bytes]}") | $(mebibytes "${metric[rust.idle_memory_bytes]}") | $(delta_percent "${metric[python.idle_memory_bytes]}" "${metric[rust.idle_memory_bytes]}") |
| Peak container memory | $(mebibytes "${metric[python.peak_memory_bytes]}") | $(mebibytes "${metric[rust.peak_memory_bytes]}") | $(delta_percent "${metric[python.peak_memory_bytes]}" "${metric[rust.peak_memory_bytes]}") |
| Runtime image size | $(mebibytes "${metric[python.image_bytes]}") | $(mebibytes "${metric[rust.image_bytes]}") | $(delta_percent "${metric[python.image_bytes]}" "${metric[rust.image_bytes]}") |

Comparative latency gates allow Rust at most Python + 25% plus a noise floor: 2 ms for health,
3 ms for metadata, 5 ms for range streaming, 10 ms for WebSocket, 25 ms for upload, and 2 seconds
for startup. Rust peak memory must stay below 85% of the 4 GiB container limit. This synthetic gate
does not replace the representative private-corpus signal/voice differential or physical-speaker test.
EOF

if (( ${#failures[@]} > 0 )); then
  {
    echo
    echo "## Failed gates"
    for failure in "${failures[@]}"; do
      echo "- $failure"
    done
  } >>"$report_path"
  cat "$report_path"
  exit 1
fi

cat "$report_path"
