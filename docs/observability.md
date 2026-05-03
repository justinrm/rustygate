# Observability

RustyGate should make gateway behavior easy to understand during local development without exposing secrets or prompt content.

## Structured Logging Plan

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

The MVP metrics set should include:

- `total_requests`
- `successful_requests`
- `failed_requests`
- `total_provider_attempts`
- `fallback_attempts`
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

Do not add these during the MVP unless explicitly requested:

- Prometheus exporter
- OpenTelemetry traces
- Distributed tracing
- External log sinks
- Dashboards
