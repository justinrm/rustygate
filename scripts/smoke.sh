#!/usr/bin/env sh

set -eu

BASE_URL="${BASE_URL:-http://127.0.0.1:8080}"

TMP_CHAT="$(mktemp)"
TMP_STATS="$(mktemp)"
cleanup() {
  rm -f "$TMP_CHAT" "$TMP_STATS"
}
trap cleanup EXIT INT TERM

check_status() {
  endpoint="$1"
  expected="$2"

  code="$(curl -sS -o /dev/null -w "%{http_code}" "$BASE_URL$endpoint")"
  if [ "$code" != "$expected" ]; then
    echo "FAIL $endpoint returned HTTP $code (expected $expected)"
    exit 1
  fi

  echo "PASS $endpoint returned HTTP $code"
}

echo "Running smoke checks against $BASE_URL"

check_status "/health" "200"
check_status "/ready" "200"

chat_code="$(curl -sS -o "$TMP_CHAT" -w "%{http_code}" \
  "$BASE_URL/v1/chat/completions" \
  -H "content-type: application/json" \
  -d '{
    "model": "mock-fast-v1",
    "messages": [
      {"role": "system", "content": "You are a concise assistant."},
      {"role": "user", "content": "Smoke check: answer with one sentence."}
    ],
    "temperature": 0.2,
    "max_tokens": 32
  }')"

if [ "$chat_code" != "200" ]; then
  echo "FAIL /v1/chat/completions returned HTTP $chat_code"
  echo "Response:"
  cat "$TMP_CHAT"
  exit 1
fi

chat_body="$(cat "$TMP_CHAT")"
case "$chat_body" in
  *"\"choices\""* ) ;;
  * )
    echo "FAIL /v1/chat/completions missing expected choices field"
    echo "Response:"
    echo "$chat_body"
    exit 1
    ;;
esac
echo "PASS /v1/chat/completions returned expected response shape"

stats_code="$(curl -sS -o "$TMP_STATS" -w "%{http_code}" "$BASE_URL/stats")"
if [ "$stats_code" != "200" ]; then
  echo "FAIL /stats returned HTTP $stats_code"
  echo "Response:"
  cat "$TMP_STATS"
  exit 1
fi

stats_body="$(cat "$TMP_STATS")"
case "$stats_body" in
  "{"* ) ;;
  * )
    echo "FAIL /stats did not return JSON"
    echo "Response:"
    echo "$stats_body"
    exit 1
    ;;
esac
echo "PASS /stats returned HTTP 200 JSON response"

echo "Smoke checks completed successfully."
