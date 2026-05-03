# Provider Routing

RustyGate keeps provider routing simple and transparent. The goal is to show clean gateway mechanics and practical fallback behavior without becoming a complex optimizer.

## Default Routing Strategy

1. Require the request to include `model`.
2. Resolve configured model aliases before provider selection.
3. Find providers whose configured model exactly matches the resolved request model.
4. Sort providers by the configured routing policy.
5. Skip providers whose circuit breaker is open.
6. Try providers in that order, retrying retryable failures on the same provider before fallback.
7. Record each provider attempt for metrics and debugging.

## Exact Model Match

Exact model matching remains the default provider eligibility rule because it is predictable and easy to test. Configured aliases are resolved to provider-specific model IDs before eligibility checks.

If a later implementation supports omitted models, it should use an explicit configured default model rather than treating every provider as eligible.

## Model Aliases

`gateway.model_aliases` maps public model IDs to configured provider model IDs:

```toml
[gateway]
model_aliases = { "gpt-4o" = "gpt-4o-mini" }
```

A request for `gpt-4o` is routed as `gpt-4o-mini`, so providers receive the model ID they are configured to serve.

## Optional Routing Policies

`gateway.routing_policy` controls candidate ordering after model eligibility:

- `priority`: lowest configured priority first.
- `cost`: lowest combined input/output token price first, then priority and provider name.
- `latency`: lowest recent average provider latency first, then priority and provider name.

The default is `priority`. Tie breakers stay deterministic so tests can assert exact provider order.

## Default Provider Fallback

Fallback should happen only after a provider returns a retryable failure. RustyGate retries the same provider first (bounded by configured retry policy), then falls back to the next matching provider. Fallback should not hide invalid client requests.

## Circuit Breakers and Probes

Open circuits are skipped during candidate execution so degraded providers stop receiving immediate traffic. After the configured open interval, a provider transitions to half-open and is allowed probe traffic. Probe success returns the provider to closed; probe failure reopens the circuit.

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
- Per-request routing hints
