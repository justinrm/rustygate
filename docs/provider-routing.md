# Provider Routing

RustyGate should start with a simple, transparent routing strategy. The MVP goal is to show clean gateway mechanics, not a complex optimizer.

## MVP Routing Strategy

1. Require the request to include `model`.
2. Find providers whose configured model exactly matches the request model.
3. Sort providers by ascending `priority`.
4. Try providers in that order.
5. Record each provider attempt for metrics and debugging.

## Exact Model Match

Exact model matching is predictable and easy to test. Avoid aliases, regex matching, dynamic model discovery, or weighted routing until the MVP is complete.

If a later implementation supports omitted models, it should use an explicit configured default model rather than treating every provider as eligible.

## Default Provider Fallback

Fallback should happen only after a provider returns a retryable failure. The MVP tries each matching provider once and does not retry the same provider. Fallback should not hide invalid client requests.

## Priority Order

Lower priority numbers should be tried first:

- `priority = 1`: primary provider
- `priority = 2`: first fallback
- `priority = 3`: second fallback

Provider priority should be deterministic so tests can assert exact behavior.

## Retryable Versus Non-Retryable Failures

Retryable failures:

- Timeout
- Rate limited
- Provider unavailable
- Temporary bad gateway style provider failures

Non-retryable failures:

- Invalid client request
- Provider authentication failure
- Unsupported model
- Provider bad response caused by an incompatible contract

## Future Routing Ideas

Do not build these until the simple strategy is working and documented:

- Weighted routing
- Latency-aware routing
- Cost-aware routing
- Circuit breakers
- Provider health probes
- Model aliases
- Per-request routing hints
