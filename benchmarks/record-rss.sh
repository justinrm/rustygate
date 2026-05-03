#!/usr/bin/env sh
set -eu

process_name="${1:?process name required}"
output="${2:?output path required}"
duration_seconds="${3:-30}"

printf 'timestamp_unix,rss_kb\n' > "$output"
end=$(( $(date +%s) + duration_seconds ))

while [ "$(date +%s)" -lt "$end" ]; do
  rss="$(pgrep -f "$process_name" | head -n 1 | xargs -r ps -o rss= -p | tr -d ' ' || true)"
  printf '%s,%s\n' "$(date +%s)" "${rss:-0}" >> "$output"
  sleep 1
done
