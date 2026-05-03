#!/usr/bin/env sh
set -eu

cd "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"

docker compose up -d --build

for url in http://127.0.0.1:8080/health http://127.0.0.1:4000/health; do
  for _ in $(seq 1 30); do
    if curl -fsS "$url" >/dev/null 2>&1; then
      break
    fi
    sleep 1
  done
done

mkdir -p results

run_oha() {
  name="$1"
  url="$2"
  oha -z 30s -c 50 -m POST \
    -H 'content-type: application/json' \
    -H 'authorization: Bearer benchmark-key' \
    -d @request.json \
    "$url" > "results/${name}.txt"
}

./record-rss.sh rustygate results/rustygate-rss.csv 35 &
rusty_rss_pid=$!
run_oha rustygate http://127.0.0.1:8080/v1/chat/completions
wait "$rusty_rss_pid" || true

./record-rss.sh litellm results/litellm-rss.csv 35 &
litellm_rss_pid=$!
run_oha litellm http://127.0.0.1:4000/v1/chat/completions
wait "$litellm_rss_pid" || true

printf 'Raw results written to benchmarks/results/.\n'
