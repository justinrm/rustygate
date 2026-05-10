#!/usr/bin/env sh
set -eu

cd "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"

mkdir -p results

duration_seconds="${DURATION_SECONDS:-30}"
concurrency="${CONCURRENCY:-50}"
runner="../target/release/rustygate_benchmark"

cargo build --release --bin rustygate_benchmark --manifest-path ../Cargo.toml

wait_for_health() {
  url="$1"
  for _ in $(seq 1 30); do
    if curl -fsS "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done

  printf 'Timed out waiting for %s\n' "$url" >&2
  return 1
}

run_scenario() {
  config_name="$1"
  workload_name="$2"
  label="${config_name}-${workload_name}"

  printf 'Running %s...\n' "$label"
  docker compose down --remove-orphans >/dev/null 2>&1 || true
  RUSTYGATE_BENCHMARK_CONFIG="/app/benchmarks/configs/${config_name}.toml" \
    docker compose up -d --build rustygate
  wait_for_health http://127.0.0.1:8080/health

  ./record-rss.sh rustygate "results/${label}-rss.csv" "$((duration_seconds + 5))" &
  rss_pid=$!
  "$runner" \
    --url http://127.0.0.1:8080/v1/chat/completions \
    --workload "workloads/${workload_name}.jsonl" \
    --duration-seconds "$duration_seconds" \
    --concurrency "$concurrency" \
    --api-key benchmark-key \
    --stats-url http://127.0.0.1:8080/stats \
    --provider-stats-url http://127.0.0.1:8080/stats/providers \
    --metrics-url http://127.0.0.1:8080/metrics \
    --output "results/${label}.json"
  wait "$rss_pid" || true
}

trap 'docker compose down --remove-orphans >/dev/null 2>&1 || true' EXIT

for config_name in mock-priority mock-prefix-affinity; do
  for workload_name in shared-prefix no-shared-prefix mixed-prompt-lengths shared-prefix-streaming no-shared-prefix-streaming; do
    run_scenario "$config_name" "$workload_name"
  done
done

docker compose images > results/compose-images.txt

printf 'Raw results written to benchmarks/results/.\n'
