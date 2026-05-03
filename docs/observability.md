# Observability

RustyGate should make gateway behavior easy to understand during local development without exposing secrets or prompt content.

## Structured Logging

RustyGate emits startup logs, HTTP tracing, and structured chat request metadata without prompt bodies by default. Request metadata logs use `tracing` fields for:

- request ID
- route
- selected provider and attempted providers
- model
- latency
- success or failure
- fallback attempts
- classified error code

Prompt content is redacted by default.

## Request IDs

Generate one request ID for each inbound request. It appears in successful chat responses (`id`), client-facing error responses (`error.request_id`), and request logging includes it in:

- request logs
- provider attempt logs
- successful request logs
- client-facing error bodies

## Metrics Collected

RustyGate exposes JSON aggregates through `/stats` and `/stats/providers`, and Prometheus-compatible text through `/metrics`.

The metrics set includes:

- `total_requests`
- `successful_requests`
- `failed_requests`
- `in_flight_requests`
- `total_provider_attempts`
- `fallback_attempts`
- `request_errors_by_category`
- `provider_errors_by_provider_and_category`
- `error_rate`
- `avg_latency_ms`
- `p95_latency_ms`
- `estimated_prompt_tokens`
- `estimated_completion_tokens`
- `requests_by_provider`
- `successes_by_provider`
- `errors_by_provider`
- `fallback_attempts_by_provider`
- `avg_latency_ms_by_provider`
- `p95_latency_ms_by_provider`
- `estimated_input_cost_usd`
- `estimated_output_cost_usd`
- `estimated_total_tokens`
- `estimated_total_cost_usd`

Prometheus metric names use the `rustygate_` prefix, including:

- `rustygate_requests_total`
- `rustygate_requests_failed_total`
- `rustygate_in_flight_requests`
- `rustygate_request_errors_total{category="..."}`
- `rustygate_provider_errors_total{provider="...",category="..."}`
- `rustygate_request_latency_ms_p95`
- `rustygate_provider_latency_ms_p95{provider="..."}`

Example scrape:

```sh
curl -H "authorization: Bearer ${RUSTYGATE_GATEWAY_API_KEY}" \
  http://127.0.0.1:8080/metrics
```

When SQLite persistence is enabled, counters and latency aggregates come from persisted request logs where possible. Live gauges such as `rustygate_in_flight_requests` always come from in-memory process state.

## Dashboard And Alert Starting Points

Recommended dashboard panels:

- Request volume, success count, failure count, and error rate.
- In-flight requests.
- Request average and p95 latency.
- Provider attempts, provider errors, and fallback attempts by provider.
- Provider p95 latency by provider.
- Timeout and rate-limit counters by provider.

Suggested initial alert thresholds for an internal demo deployment:

- Error rate above 5% for 5 minutes.
- Any provider timeout or rate-limit counter increasing quickly for 5 minutes.
- Request p95 latency above the expected upstream timeout budget for 5 minutes.
- Fallback attempts increasing while primary provider errors increase.

## Prompt Logging Policy

Do not log prompt content by default. The `log_prompt_content` setting is local-development-only; when enabled, startup logs make that setting obvious and SQLite request logs can include prompt messages.

## Persistence

In-memory metrics remain the default lightweight storage path. Optional SQLite persistence stores request logs and provider attempts when `[storage].enabled = true`, and stats endpoints can read their aggregates from SQLite in that mode.

## Provider-Level Stats

Provider stats should help answer:

- Which provider handled the request?
- Which provider failed?
- How often did fallback occur?
- Which provider is slow?
- What was the estimated cost per provider?

## Future Ideas

Do not add these unless they are part of an explicit post-`v0.1` roadmap:

- OpenTelemetry traces
- Distributed tracing
- External log sinks
- Dashboards
