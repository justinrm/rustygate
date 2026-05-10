# Provider Routing

RustyGate keeps provider routing simple and transparent. The goal is to show clean gateway mechanics, practical fallback behavior, and a small inference-aware routing slice without becoming a complex optimizer.

For `v0.4` inference-aware routing context and terminology boundaries, see `docs/inference-aware-routing.md`.

## Default Routing Strategy

1. Require the request to include `model`.
2. Resolve configured model aliases before provider selection.
3. If the resolved model matches a configured model pool, select providers from that pool's members.
4. Otherwise, find providers whose configured model exactly matches the resolved request model.
5. Sort providers by the configured routing policy (or a pool-specific override when configured).
6. Skip providers whose circuit breaker is open.
7. Try providers in that order, retrying retryable failures on the same provider before fallback.
8. Record each provider attempt for metrics and debugging.

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

Aliases can also target a pool public model ID (`model_pools[].name` or one of its `aliases`).

## Model Pools

`model_pools` groups replicas under one public model surface without treating each replica as an unrelated fallback provider:

```toml
model_aliases = { "mock-fast" = "mock-fast-pool" }

[[model_pools]]
name = "mock-fast-pool"
aliases = ["mock-fast-ha"]
members = ["mock-low-latency", "mock-backup"]
```

- `name` and `aliases` define public model IDs.
- `members` reference configured provider names.
- Optional `routing_policy` overrides `gateway.routing_policy` for that pool only.
- Optional `max_in_flight` sets an admission cap for the pool as a whole.

Provider-only config remains valid when `model_pools` is omitted.

Pool IDs cannot conflict with `gateway.model_aliases` keys or configured provider model IDs. A gateway alias can point at a pool public ID, so clients can keep using stable names while the gateway maps them to a pool.

Within a pool, the member list is the eligibility boundary. Fallback stays inside the pool unless the request resolves to a non-pooled model through the provider-only path.

## Optional Routing Policies

`gateway.routing_policy` controls candidate ordering after model eligibility:

- `priority`: lowest configured priority first.
- `cost`: lowest combined input/output token price first, then priority and provider name.
- `latency`: lowest bounded recent provider p95 latency first, falling back to average latency when p95 is unavailable, then priority and provider name.
- `prefix_affinity`: for multi-member model pools, prefer the recent healthy replica associated with a high-confidence prompt-prefix fingerprint while load remains balanced.

The default is `priority`. Tie breakers stay deterministic so tests can assert exact provider order.

Latency routing also applies small local penalties for active in-flight load, approximate queue pressure, recent provider errors, and degraded circuit state. These signals are gateway-local heuristics; they do not claim to know GPU queues or KV-cache residency inside the model runtime.

## Prefix Affinity

`prefix_affinity` is available globally through `gateway.routing_policy` or as a pool-specific `model_pools[].routing_policy` override. It only applies to model pools with multiple members and high-confidence prefix fingerprints. Provider-only routing, single-member pools, missing fingerprints, and low-confidence requests fall back to the configured `gateway.prefix_affinity.fallback_policy`.

```toml
[gateway]
routing_policy = "prefix_affinity"

[gateway.prefix_affinity]
ttl_seconds = 300
max_entries = 10000
load_imbalance_threshold = 2
fallback_policy = "latency"

[[model_pools]]
name = "mock-fast-pool"
routing_policy = "prefix_affinity"
members = ["mock-low-latency", "mock-backup"]
```

The affinity index stores only hashed prefix fingerprints and provider names. It is bounded, expires entries by TTL, and is local to one RustyGate process. It improves placement heuristics for shared-prefix workloads but does not prove that a model runtime kept or reused KV-cache blocks.

When a prefix has a previous provider selection, RustyGate keeps using that provider while its in-flight and queue-pressure deltas stay within `load_imbalance_threshold`. If the preferred provider has an open circuit or the pool is imbalanced, routing falls back to healthier candidates. New high-confidence prefixes are spread deterministically across healthy pool members.

Prefix-affinity routing records decision reasons through `rustygate_routing_decisions_total{policy="prefix_affinity",reason="..."}`:

- `prefix_hit`: a previous healthy provider was reused for the same hashed prefix.
- `prefix_miss`: a new high-confidence prefix was placed deterministically.
- `load_imbalanced`: affinity was overridden because the preferred provider was too loaded.
- `circuit_open`: affinity or fallback candidates were affected by open circuit state.
- `fallback`: prefix routing could not apply and the configured fallback policy ordered candidates.
- `selected`: a provider attempt ultimately succeeded.

Tie breakers remain deterministic. Cost and latency policies sort by their primary signal, then stable provider priority and provider name. Prefix misses use deterministic placement across healthy pool members rather than random assignment.

## Experimental Runtime Signals

The `runtime-cache-signals` Cargo feature contains a mock-backed spike for precise runtime-signal-aware routing. It is disabled by default, has no production adapter, and is not required for normal `priority`, `cost`, `latency`, or `prefix_affinity` routing.

The spike models runtime worker identity, queue depth, in-flight load, KV-cache utilization, cache hit fraction, and optional hashed prefix residency. It exists to prove how routing scores could use real runtime signals later; it does not make RustyGate control KV allocation, eviction, batching, or GPU scheduling.

## `/v1/models` and Public IDs

`/v1/models` continues listing non-pooled provider model IDs and gateway aliases.

When model pools are configured, it also lists pool public IDs (`name` and `aliases`) and avoids exposing internal replica model IDs when those replicas are only reachable through a pool.

## Default Provider Fallback

Fallback should happen only after a provider returns a retryable failure. RustyGate retries the same provider first (bounded by configured retry policy), then falls back to the next matching provider. Fallback should not hide invalid client requests.

## Circuit Breakers and Probes

Open circuits are penalized during latency ordering and skipped during candidate execution so degraded providers stop receiving immediate traffic. After the configured open interval, a provider transitions to half-open and is allowed probe traffic. Probe success returns the provider to closed; probe failure reopens the circuit.

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
