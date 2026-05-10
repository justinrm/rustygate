# Inference-Aware Routing

RustyGate `v0.4` focuses on one practical bottleneck in self-hosted LLM serving: repeated prefill work caused by weak prompt-prefix locality and uncontrolled overload.

This document describes what the gateway can do today in principle, what it cannot do without runtime integration, and how the implementation should evolve in small, testable slices.

## Why This Exists

Many internal workloads repeatedly send long shared prompt prefixes:

- stable system prompts
- repeated tool schemas
- recurring RAG context headers
- multi-turn chat with a mostly stable lead-in context

When those requests are distributed randomly across replicas, each replica may repeat expensive prefill work and miss cache locality opportunities. `v0.4` aims to improve request placement and overload behavior at the gateway edge.

## Practical Concepts

### Prefill and Decode

- **Prefill**: the model processes existing prompt tokens and builds intermediate attention/KV state before generating new tokens.
- **Decode**: the model generates new tokens step by step after prefill is complete.

User-perceived latency is often dominated by prefill for long prompts and by decode for long generations.

### KV Cache and Prefix Cache

- **KV cache** is runtime-side state that stores attention keys/values for previously processed tokens.
- **Prefix cache** is a practical effect where repeated leading prompt segments can reduce repeated prefill cost when routed to the same replica and retained in runtime state.

RustyGate does not directly control KV memory. It can only influence whether related requests are likely to land on the same replica.

### Time To First Token (TTFT)

**TTFT** measures the delay from request admission to first streamed token. It is a high-value signal for interactive UX and a useful proxy for prefill and queue pressure effects.

RustyGate records provider-level TTFT for streaming requests without storing prompt text. The first `v0.4` load-aware slice treats configured provider names as replica identities, which matches model-pool members in local self-hosted deployments.

## Target Workload Patterns

The first `v0.4` routing work optimizes for workloads where reusable prefix material is common:

- repeated instruction-heavy system prompts
- repeated tool definition blocks and tool-choice shapes
- repeated retrieved context scaffolding before a user-specific suffix
- multi-turn sessions that share stable conversation framing

Requests with no stable prefix should remain on normal deterministic routing paths.

## Implementation Progression

### Completed Slice: Load-Aware Replica State

RustyGate now tracks process-local provider load signals that future admission and prefix-affinity routing can reuse:

- per-provider in-flight request counts
- bounded provider latency and TTFT samples
- recent provider errors by category
- approximate queue pressure from active requests and token estimates
- routing-decision counters by policy and reason
- circuit state in provider stats

These are gateway-side observations. They are useful for routing and dashboards, but they are not runtime GPU queue depth, KV utilization, or true cache-hit telemetry.

### Completed Slice: Admission Control

RustyGate now applies immediate admission checks after authentication and request validation:

- global in-flight request cap
- model-pool in-flight cap
- provider in-flight cap
- estimated prompt-token and total-token budgets

Concurrency caps return `503 admission_rejected` with `Retry-After` because they represent temporary capacity pressure. Estimated token-budget failures return `400 invalid_request` because the request is too large for the configured gateway policy. The implementation intentionally does not add background queues; this avoids unbounded request buffering and keeps overload visible to clients.

Admission counters are local to one RustyGate process and use the existing heuristic token estimator. They do not represent GPU memory pressure, scheduler queue depth, or actual KV-cache residency.

### Completed Slice: Prefix Fingerprinting

RustyGate now computes a privacy-preserving fingerprint for requests with reusable leading prompt material. The fingerprint input is a short-lived, versioned canonical payload that includes the resolved model ID, normalized stable leading messages, tool schemas and tool-choice shape, parallel tool-call behavior, and response format when present.

The implementation excludes the final user message as a volatile suffix when possible. Requests with no reusable material are marked low-confidence and skipped for future prefix-affinity routing. The returned metadata contains only the SHA-256 digest, confidence, estimated prefix characters, and heuristic prefix tokens; raw prefix text is not logged, persisted, or returned in public API responses.

Prefix fingerprint outcomes are exposed only as aggregates through `prefix_fingerprints_by_outcome` and `rustygate_prefix_fingerprints_total{outcome="hit|miss"}`.

### Completed Slice: Heuristic Prefix Affinity

RustyGate now uses the gateway-local fingerprint heuristic when `prefix_affinity` is configured:

1. Prefer the most recent healthy replica used for a high-confidence fingerprint when load is balanced.
2. Fall back to latency/priority behavior when prefix confidence is low or load is imbalanced.
3. Pair with explicit admission control so overload is rejected cleanly instead of creating hidden queue collapse.

The affinity index maps hashed prefix fingerprints to recent provider selections. Entries are bounded by capacity, expire by TTL, and are process-local. New high-confidence prefixes are spread deterministically across healthy pool members; repeated prefixes stick to the previous healthy member until in-flight or queue-pressure imbalance exceeds the configured threshold.

This pass is intentionally heuristic. It does not consume runtime cache events and should not be described as true KV-cache-aware routing.

### Spike: Runtime-Signal-Aware Routing

Milestone 7 investigates whether RustyGate should consume runtime-level cache and queue signals from self-hosted workers. The goal is to define the shape of a possible integration without making normal gateway operation depend on a model-runtime protocol.

Supported runtime options today fall into a few practical categories:

- **vLLM automatic prefix caching**: vLLM can reuse KV state for prompts that share leading token blocks when prefix caching is enabled in the runtime. RustyGate can improve the chance of reuse by routing repeated prefixes to the same worker, but the runtime still owns allocation, eviction, and block matching.
- **vLLM KV-cache events**: vLLM exposes experimental event streams for stored, removed, and cleared KV blocks. These can represent precise prefix/block residency, but consuming them would require event subscription, replay handling, version compatibility, and a mapping between RustyGate prefix fingerprints and runtime block hashes.
- **vLLM Prometheus metrics**: vLLM exposes runtime metrics such as running and waiting request counts, queue time, KV-cache usage, prefix-cache queries, prefix-cache hits, TTFT, prefill time, and decode time. These are useful runtime health signals, but they are aggregate metrics rather than per-request cache-residency guarantees.
- **OpenAI-compatible local servers without cache signals**: many local servers expose only OpenAI-shaped request/response APIs. For these, RustyGate should stay with gateway-local prefix affinity, admission control, TTFT, in-flight counts, and fallback behavior.

The first implementation should define a feature-gated runtime-signal interface and a mock signal source only. It should be able to represent:

- worker identity, mapped to configured provider or pool-member names
- queue depth or runtime in-flight load
- KV-cache utilization
- prefix-cache hit fraction
- optional prefix or block residency when precise events are available

This trait must stay mockable and must not store raw prompt text. Prefix residency, when represented, should use hashed or runtime-provided identifiers rather than prompt content.

#### Experimental Cargo Feature

Runtime-signal code is experimental and should be compiled only with the `runtime-cache-signals` Cargo feature. The feature is disabled by default and is not required for normal provider routing, prefix affinity, admission control, metrics, or OpenAI-compatible request handling.

Use it for local tests and design experiments only:

```sh
cargo test --features runtime-cache-signals runtime_signal
```

#### Decision

The milestone 7 decision is **No-Go for a real runtime adapter in `v0.4`**. vLLM exposes promising automatic prefix caching, KV-cache event streams, and Prometheus runtime metrics, but a production-shaped adapter would add protocol dependencies and operational assumptions that are better justified after the benchmark milestone proves the value of more precise signals.

RustyGate should stop here at a mock-backed, feature-gated spike. A future adapter can be reconsidered if benchmarks show heuristic prefix affinity is insufficient and the target runtime exposes stable, documented cache and queue signals.

## What The Gateway Can And Cannot Influence

- **Can influence:** request routing choices, admission and queueing policy, retry/fallback behavior, and observability.
- **Cannot influence without runtime integration:** KV allocation/eviction, scheduler-level batching behavior, GPU scheduling, and prefill/decode execution internals.

## Terminology Boundaries

Use these terms precisely in docs and reviews:

- **Exact-match response cache**: returns previously stored deterministic full responses for identical requests.
- **Semantic cache**: reuses responses by embedding similarity, with correctness/privacy tradeoffs.
- **Prefix affinity**: routing heuristic that tries to keep shared-prefix requests on the same replica.
- **KV-cache-aware routing**: runtime-signal-driven routing based on actual runtime cache/load telemetry.

These are related but not equivalent mechanisms.

