# Failure Handling

RustyGate should classify failures clearly and return clean JSON errors. Client-facing errors must not expose secrets, raw provider payloads, stack traces, or Authorization headers.

OpenAI compatibility endpoints use the same internal error categories and status mappings as chat completions. Streaming Responses emit SSE `error` events when a provider fails after the HTTP response has started.

## Error Categories

- `InvalidRequest`: request shape or validation failure
- `Timeout`: provider did not respond within the configured timeout
- `RateLimited`: provider returned a rate limit response
- `AuthenticationFailed`: provider credentials failed
- `ProviderUnavailable`: provider is temporarily unavailable
- `ProviderBadResponse`: provider returned malformed or unexpected data
- `NoProviderAvailable`: routing could not find a usable provider
- `Internal`: unexpected gateway failure

## Status Code Mappings

- `400`: invalid request
- `401`: missing or invalid gateway bearer token
- `429`: gateway or provider rate limit
- `502`: upstream provider error
- `503`: no provider available
- `504`: timeout
- `500`: unknown internal failure

## Compatibility Surface

The gateway keeps one error classification model across `/v1/chat/completions`, `/v1/responses`, and the broader OpenAI-shaped endpoint families. New endpoints should map validation failures to `400`, gateway auth failures to `401`, gateway/provider rate limits to `429`, upstream failures to `502`, no-provider routing failures to `503`, and provider timeouts to `504`.

## Retry Policy

Retry only when the failure is likely temporary. RustyGate retries the same provider first (bounded by `max_retries` and backoff settings), then falls back to the next matching provider.

Configured retry controls:

- `gateway.max_retries`: default per-provider retry count
- `gateway.retry.initial_backoff_ms`: initial retry delay
- `gateway.retry.max_backoff_ms`: cap for exponential retry delay
- `gateway.retry.jitter_ms`: deterministic jitter upper bound
- Provider overrides: `max_retries`, `retry_initial_backoff_ms`, `retry_max_backoff_ms`, `retry_jitter_ms`

Retry candidates:

- Timeout
- Rate limited, when another matching provider is available
- Provider unavailable

## Fallback Policy

Fallback should try the next matching provider in priority order after retryable failures are exhausted on the current provider. Fallback should record which providers were attempted and why each attempt failed.

## Circuit Breaker Policy

Circuit breakers protect degraded providers from immediate repeated traffic.

- State machine: `Closed` -> `Open` -> `HalfOpen` -> `Closed`
- `Closed`: provider receives traffic normally
- `Open`: provider is skipped until cooldown expires
- `HalfOpen`: limited probe traffic is allowed
- Probe success closes the circuit; probe failure reopens it

Configured circuit-breaker controls:

- `gateway.circuit_breaker.failure_threshold`
- `gateway.circuit_breaker.open_duration_ms`
- `gateway.circuit_breaker.half_open_max_probes`
- Provider overrides: `circuit_breaker_failure_threshold`, `circuit_breaker_open_duration_ms`, `circuit_breaker_half_open_max_probes`

## What Not To Retry

Do not retry:

- Invalid request payloads
- Authentication failures
- Missing provider configuration
- Unsupported models
- Responses that cannot be safely normalized

## Secret Redaction Rules

- Never log API keys.
- Never log Authorization headers.
- Never log prompt content by default.
- Never return full provider raw errors to clients.
- Prefer request IDs and classified errors for debugging.
