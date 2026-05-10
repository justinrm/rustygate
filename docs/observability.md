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
- admission rejection reason when capacity or estimated-token checks reject a request

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
- `admission_rejections_by_reason`
- `provider_errors_by_provider_and_category`
- `recent_provider_errors_by_provider_and_category`
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
- `in_flight_requests_by_provider`
- `p50_ttft_ms_by_provider`
- `p95_ttft_ms_by_provider`
- `queue_pressure_by_provider`
- `routing_decisions_by_policy_and_reason`
- `prefix_fingerprints_by_outcome`
- `circuit_state_by_provider`
- `stream_outcomes_by_outcome`
- `p95_stream_duration_ms`
- `estimated_input_cost_usd`
- `estimated_output_cost_usd`
- `estimated_total_tokens`
- `estimated_total_cost_usd`

Prometheus metric names use the `rustygate_` prefix, including:

- `rustygate_requests_total`
- `rustygate_requests_failed_total`
- `rustygate_in_flight_requests`
- `rustygate_request_errors_total{category="..."}`
- `rustygate_admission_rejections_total{reason="..."}`
- `rustygate_provider_errors_total{provider="...",category="..."}`
- `rustygate_request_latency_ms_p95`
- `rustygate_provider_latency_ms_p95{provider="..."}`
- `rustygate_provider_in_flight_requests{provider="..."}`
- `rustygate_provider_ttft_ms_p50{provider="..."}`
- `rustygate_provider_ttft_ms_p95{provider="..."}`
- `rustygate_provider_queue_pressure{provider="..."}`
- `rustygate_routing_decisions_total{policy="...",reason="..."}`
- `rustygate_prefix_fingerprints_total{outcome="hit|miss"}`
- `rustygate_stream_outcomes_total{outcome="completed|mid_stream_failure|idle_timeout|incomplete|cancelled"}`
- `rustygate_stream_duration_ms_p95`

Example scrape:

```sh
curl -H "authorization: Bearer ${RUSTYGATE_GATEWAY_API_KEY}" \
  http://127.0.0.1:8080/metrics
```

Prefix-affinity routing uses `rustygate_routing_decisions_total{policy="prefix_affinity",reason="..."}` with reasons such as `prefix_hit`, `prefix_miss`, `load_imbalanced`, `circuit_open`, `fallback`, and `selected`. These are routing outcomes, distinct from `rustygate_prefix_fingerprints_total`, which only reports whether a request had reusable high-confidence prefix material.

When SQLite persistence is enabled, counters and latency aggregates come from persisted request logs where possible. Live gauges and process-local counters such as `rustygate_in_flight_requests`, provider in-flight counts, provider queue pressure, TTFT summaries, stream outcome/duration metrics, routing-decision counters, prefix-fingerprint counters, prefix-affinity counters, and admission-rejection counters always come from in-memory process state.

The v0.4 inference-aware metrics are gateway-local signals. Provider in-flight counts, queue pressure, prefix-affinity decisions, and admission rejections describe RustyGate's view of request handling; they are not GPU scheduler metrics, runtime queue depth, or KV-cache residency.

## Streaming Observability

Streaming requests report provider TTFT separately from full stream duration. TTFT measures time until the selected provider produces the first stream event. Stream duration measures how long the client-facing SSE body lived before completion, timeout, mid-stream failure, incomplete upstream EOF, or observable downstream cancellation.

The route layer forwards chat streaming chunks without collecting the full assistant response. Responses streaming keeps an output buffer only so it can emit the final `response.completed` object, and that buffer is capped. Provider SSE parsers also cap buffered event data and accumulated completion text.

## Runtime Signal Boundaries

The experimental `runtime-cache-signals` feature defines mockable runtime cache/load signals for routing-score tests, but RustyGate does not scrape or export vLLM KV-cache metrics yet. Runtime metrics such as vLLM queue depth, prefix-cache hits, and KV utilization should be observed directly from the model runtime until a future adapter is explicitly implemented.

## Dashboard And Alert Starting Points

Recommended dashboard panels:

- Request volume, success count, failure count, and error rate.
- In-flight requests.
- Provider in-flight requests and queue pressure by replica.
- Request average and p95 latency.
- Streaming TTFT p95 by provider.
- Provider attempts, provider errors, and fallback attempts by provider.
- Provider p95 latency by provider.
- Timeout and rate-limit counters by provider.
- Stream outcomes and p95 stream duration.
- Routing decisions by policy and reason.
- Prefix fingerprint hit and miss outcomes.
- Prefix-affinity hit, miss, fallback, load-imbalance, and circuit-open decisions.
- Admission rejections by reason.

These are suggested panels only. RustyGate ships Prometheus-compatible metrics, not a dashboard package.

Suggested initial alert thresholds for an internal demo deployment:

- Error rate above 5% for 5 minutes.
- Any provider timeout or rate-limit counter increasing quickly for 5 minutes.
- Admission rejections increasing quickly for 5 minutes.
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
- Which provider is currently busy?
- Which streaming provider has high TTFT?
- Which routing policy and reason selected traffic?
- What was the estimated cost per provider?

## Future Ideas

Do not add these unless they are part of an explicit future roadmap:

- External log sinks
- Packaged dashboard definitions
- Runtime-specific KV-cache metric adapters
