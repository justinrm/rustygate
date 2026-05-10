# Failure Handling

RustyGate should classify failures clearly and return clean JSON errors. Client-facing errors must not expose secrets, raw provider payloads, stack traces, or Authorization headers.

OpenAI compatibility endpoints use the same internal error categories and status mappings as chat completions. Streaming Responses emit SSE `error` events when a provider fails after the HTTP response has started.

## Error Categories

- `InvalidRequest`: request shape or validation failure
- `AdmissionRejected`: temporary gateway capacity pressure from configured concurrency caps
- `Timeout`: provider did not respond within the configured timeout
- `RateLimited`: provider returned a rate limit response
- `AuthenticationFailed`: provider credentials failed
- `ProviderUnavailable`: provider is temporarily unavailable
- `ProviderBadResponse`: provider returned malformed or unexpected data
- `NoProviderAvailable`: routing could not find a usable provider
- `Internal`: unexpected gateway failure

## Status Code Mappings

- `400`: invalid request, including estimated token-budget violations
- `401`: missing or invalid gateway bearer token
- `429`: gateway or provider rate limit
- `502`: upstream provider error
- `503`: no provider available or temporary admission-capacity rejection
- `504`: timeout
- `500`: unknown internal failure

## Compatibility Surface

The gateway keeps one error classification model across `/v1/chat/completions`, `/v1/responses`, and the broader OpenAI-shaped endpoint families. New endpoints should map validation failures to `400`, gateway auth failures to `401`, gateway/provider rate limits to `429`, upstream failures to `502`, no-provider routing failures and temporary admission-capacity rejections to `503`, and provider timeouts to `504`.

## Admission Control

Admission control runs after authentication, rate limiting, body parsing, and request validation. It rejects overload before routing can create hidden provider queues.

Configured admission controls:

- `gateway.admission.max_global_in_flight`
- `gateway.admission.max_estimated_prompt_tokens`
- `gateway.admission.max_estimated_total_tokens`
- `gateway.admission.retry_after_seconds`
- Provider `max_in_flight`
- Model-pool `max_in_flight`

Concurrency-cap rejections return `503` with `Retry-After` and `error.code = "admission_rejected"`. Estimated token-budget rejections return `400` with `error.code = "invalid_request"`. RustyGate does not queue rejected requests in this implementation.

Admission rejection reasons are recorded separately for logs and metrics:

- `global_in_flight_limit`: current gateway-wide in-flight requests reached `gateway.admission.max_global_in_flight`.
- `pool_in_flight_limit`: the resolved model pool reached its `max_in_flight` cap.
- `provider_in_flight_limit`: every otherwise eligible provider is at its provider-level `max_in_flight` cap.
- `max_estimated_prompt_tokens`: the estimated prompt size exceeded `gateway.admission.max_estimated_prompt_tokens`.
- `max_estimated_total_tokens`: estimated prompt plus requested completion tokens exceeded `gateway.admission.max_estimated_total_tokens`.

Admission rejections are distinct from gateway rate limits and provider rate limits. Rate limits describe request frequency; admission describes configured request size or live capacity pressure.

## Retry Policy

Retry only when the failure is likely temporary. RustyGate retries the same provider first (bounded by `max_retries` and backoff settings), then falls back to the next matching provider.

Configured retry controls:

- `gateway.max_retries`: default per-provider retry count
- `gateway.stream_idle_timeout_ms`: maximum idle gap between streamed provider chunks after a stream has started
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

Under `prefix_affinity`, fallback still uses the normal retry and fallback machinery. Low-confidence prefixes, missing affinity entries, load imbalance, and open circuits can all route through the configured `gateway.prefix_affinity.fallback_policy` before provider execution. Once a provider attempt starts, retryable provider failures are handled the same way as priority, cost, or latency routing.

## Streaming Failures

For streaming chat completions and Responses, RustyGate waits for the first provider event before returning the client SSE response. Once the stream is active, `gateway.stream_idle_timeout_ms` bounds the maximum idle gap between upstream chunks. If the provider stalls longer than that, RustyGate emits a clean SSE error event, records the error category as `timeout`, records the stream outcome as `idle_timeout`, and does not emit `[DONE]`.

Provider errors after the first chunk are emitted as SSE error events and recorded as mid-stream failures. An upstream stream that ends without a completion event is treated as `ProviderBadResponse`, not a successful completion. Admission rejections still happen before the stream starts and keep their existing `admission_rejected` classification.

Client disconnects are handled by dropping the downstream SSE body. RustyGate keeps admission and in-flight guards owned by the stream generator so those counters release when the body is dropped; this is best-effort cancellation propagation and does not require RustyGate to keep reading from the upstream stream after the client is gone.

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
