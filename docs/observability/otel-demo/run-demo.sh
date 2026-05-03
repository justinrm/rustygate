#!/usr/bin/env sh
set -eu

demo_dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
repo_root="$(CDPATH= cd -- "${demo_dir}/../../.." && pwd)"

cd "$repo_root"

docker compose -f docs/observability/otel-demo/docker-compose.yml up -d --build

printf 'Waiting for RustyGate...\n'
for _ in $(seq 1 30); do
  if curl -fsS http://127.0.0.1:8080/health >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

curl -fsS http://127.0.0.1:8080/v1/chat/completions \
  -H 'content-type: application/json' \
  -H 'authorization: Bearer local-dev-gateway-key' \
  -d '{
    "model": "mock-fast-v1",
    "messages": [
      {"role": "user", "content": "Generate a short trace demo response."}
    ],
    "temperature": 0,
    "max_tokens": 64
  }' >/dev/null

printf 'Trace UI: http://127.0.0.1:16686/search?service=rustygate\n'
curl -fsS 'http://127.0.0.1:16686/api/traces?service=rustygate&limit=1' \
  -o docs/observability/otel-demo/latest-trace.json || true
