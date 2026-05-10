# TODO.md

## RustyGate v0.4 Plan

RustyGate is now a lightweight internal/demo inference gateway with completed MVP, `v0.2` OpenAI compatibility, and `v0.3` portfolio-hardening work. The next version should not broaden into a general-purpose gateway. `v0.4` should make the project more interesting by targeting a real inference-serving bottleneck: wasteful prefill and poor cache locality for self-hosted LLM replicas.

The core idea is to evolve RustyGate from "pick a provider and fall back if it fails" into a small, testable, inference-aware edge:

- Keep the existing OpenAI-compatible request surface.
- Keep provider abstraction, fallback, auth, metrics, and docs.
- Add a bounded model-replica layer for self-hosted OpenAI-compatible workers.
- Add admission control so overload is explicit instead of accidental.
- Add prefix-aware routing that can improve KV/prefix-cache locality when workloads share long prompts.
- Measure time-to-first-token, queue pressure, fallback behavior, and gateway overhead honestly.

`v0.4` should remain a portfolio-grade, production-shaped project. It should not claim to manage GPU memory or KV cache directly unless RustyGate is consuming real runtime signals from a model server.

## Completed Baseline

- [x] `GET /health` and `GET /ready`
- [x] `POST /v1/responses` and `POST /v1/chat/completions`
- [x] Real outbound provider support for `openai_compatible` and `anthropic`
- [x] Mock providers for deterministic tests
- [x] Provider selection by exact model match and priority
- [x] Optional cost-aware and latency-aware routing policies
- [x] Same-provider retries with bounded backoff and jitter
- [x] Fallback across eligible providers for retryable errors
- [x] Provider circuit breakers with half-open recovery
- [x] Provider health probes feeding `/ready`
- [x] SSE streaming for mock, OpenAI-compatible, and Anthropic paths
- [x] Structured request metadata logs with prompt redaction by default
- [x] In-memory and optional SQLite request metrics
- [x] Prometheus-compatible `/metrics`
- [x] OpenTelemetry trace export with provider-attempt spans
- [x] Multi-key auth with hashed SQLite storage, roles, quotas, and `rustygate_admin`
- [x] Bounded local rate-limit state and optional Redis-backed rate limiting
- [x] Request body, message count, and content length limits
- [x] Tool/function calling support
- [x] Opt-in exact-match response caching
- [x] Experimental semantic cache primitives behind `semantic-cache`
- [x] Benchmark harness for gateway overhead
- [x] Dockerfile, config profiles, CI, smoke checks, and operations docs

## Scope Rules

- [ ] Keep `v0.4` focused on self-hosted model pools and gateway-side inference controls.
- [ ] Prefer small config additions over broad rewrites.
- [ ] Keep route handlers free of provider-selection logic.
- [ ] Keep OpenAI-compatible request and response shapes stable unless a task explicitly changes them.
- [ ] Do not add Kubernetes manifests, dashboards, billing, or a production policy engine.
- [ ] Do not claim true KV-cache control unless the gateway consumes real model-server cache signals.
- [ ] Document every new production-shaped feature with its limits and failure modes.

## How To Use This Checklist

Work from top to bottom. Each milestone should leave the project runnable, tested, and easier to explain.

For each implementation slice:

1. Make the smallest code change that completes the next unchecked task.
2. Add or update focused tests for the behavior.
3. Run `cargo fmt`.
4. Run the most relevant focused tests.
5. Run `cargo clippy --all-targets --all-features` and `cargo test` before a larger handoff.
6. Update docs and example config whenever config shape, API shape, metrics, or operational behavior changes.

## Milestone 1: Define the v0.4 Inference-Aware Contract

Goal: make the new direction explicit before changing routing internals.

- [x] Update `docs/roadmap.md` with a `v0.4` section.
  - [x] State that `v0.4` targets prefix/cache-locality routing and admission control.
  - [x] State that RustyGate remains a gateway edge, not an inference runtime.
  - [x] List what the gateway can influence: routing, queueing, admission, retries, fallback, observability.
  - [x] List what the gateway cannot influence without runtime integration: actual KV allocation, eviction, batching, GPU scheduling, and prefill/decode execution.
- [x] Update `README.md` current status and known limitations.
  - [x] Add a short note about `v0.4` being planned, not implemented.
  - [x] Clarify that exact-match response caching is not the same as KV-cache reuse.
  - [x] Clarify that existing benchmarks measure gateway overhead, not inference-runtime latency.
- [x] Add a new design doc, `docs/inference-aware-routing.md`.
  - [x] Explain prefill, decode, KV cache, prefix cache, and time-to-first-token in practical terms.
  - [x] Describe the target workload: repeated system prompts, tool schemas, RAG context, and multi-turn chats.
  - [x] Describe the first implementation as heuristic prefix affinity.
  - [x] Describe a later implementation as precise KV-cache-aware routing if runtime signals are available.
- [x] Add a short "not competing with general gateways" section.
  - [x] Preserve the project boundary: small, documented, testable, Rust-focused.
  - [x] Call out large-gateway features that remain out of scope.

Acceptance check:

- [x] A reviewer can read the docs and understand why `v0.4` exists.
- [x] The docs distinguish response caching, semantic caching, prefix affinity, and true KV-cache-aware routing.

## Milestone 2: Model Pools and Replica Configuration

Goal: represent multiple replicas of the same model without treating every replica as a separate fallback provider.

- [x] Extend config with an explicit model-pool concept.
  - [x] Add a `model_pools` or equivalent config section.
  - [x] Define pool name, public model aliases, routing policy, and member providers.
  - [x] Keep the old provider-only config working for existing examples.
  - [x] Validate that each pool has at least one member.
  - [x] Validate that each member references an existing provider.
  - [x] Validate that pool aliases do not conflict with existing model aliases.
- [x] Add internal model-pool structs.
  - [x] Create a model-pool module under `src/routing` or a clearly named new module.
  - [x] Represent a pool separately from provider priority/fallback ordering.
  - [x] Track whether a provider is a primary, fallback-only, or replica member if that distinction is needed.
  - [x] Keep route handlers unaware of the pool internals.
- [x] Update provider candidate selection.
  - [x] Resolve aliases before pool selection.
  - [x] Select eligible pool members for the requested model.
  - [x] Preserve current behavior when no pool config is present.
  - [x] Keep deterministic tie breakers for tests.
- [x] Update `GET /v1/models`.
  - [x] Show public model IDs from aliases and pools.
  - [x] Avoid exposing internal replica names unless intentionally documented.
- [x] Add config examples.
  - [x] Add a local mock pool with two or three replicas.
  - [x] Add a self-hosted OpenAI-compatible pool example.
  - [x] Keep production config conservative.

Acceptance check:

- [x] Existing configs continue to load.
- [x] A model pool with multiple mock replicas routes requests successfully.
- [x] Unit tests cover alias resolution, pool validation, and deterministic candidate ordering.

## Milestone 3: Load-Aware Replica State

Goal: collect enough local state to make better routing and admission decisions without pretending to know GPU internals.

- [x] Add replica-level state.
  - [x] Track in-flight requests per provider or pool member.
  - [x] Track recent request latency per replica.
  - [x] Track recent time-to-first-token for streaming requests.
  - [x] Track recent provider errors by category.
  - [x] Track circuit state alongside replica state in responses where useful.
- [x] Improve latency tracking for routing.
  - [x] Replace or supplement simple average latency with bounded rolling samples, EWMA, or p95.
  - [x] Prefer recent samples over all-time averages.
  - [x] Penalize replicas with recent errors or open circuits.
  - [x] Keep metric storage bounded.
- [x] Add queue-pressure approximations.
  - [x] Use in-flight count as the first queue/load signal.
  - [x] Optionally estimate prompt-size pressure from request token estimates.
  - [x] Optionally estimate decode pressure from `max_tokens` or `max_output_tokens`.
  - [x] Do not block on precise tokenizer integration for the first pass.
- [x] Expose new metrics.
  - [x] `rustygate_provider_in_flight_requests{provider="..."}`
  - [x] `rustygate_provider_ttft_ms_p95{provider="..."}`
  - [x] `rustygate_provider_queue_pressure{provider="..."}`
  - [x] `rustygate_routing_decisions_total{policy="...",reason="..."}`
- [x] Update stats endpoints.
  - [x] Add per-provider in-flight counts.
  - [x] Add TTFT summaries when streaming requests are present.
  - [x] Add selected routing reason if feasible without storing prompt content.

Acceptance check:

- [x] Metrics show per-replica load while requests are in flight.
- [x] Streaming paths record TTFT without logging prompt content.
- [x] Tests cover in-flight increment/decrement on success, failure, and early streaming errors.

## Milestone 4: Admission Control

Goal: reject or defer overload deliberately before requests create bad tail latency or exhaust upstream capacity.

- [x] Add configurable concurrency limits.
  - [x] Add global max in-flight requests.
  - [x] Add per-model-pool max in-flight requests.
  - [x] Add per-provider max in-flight requests.
  - [x] Add per-key override support only if it fits the existing key-limit model cleanly.
- [x] Add request budget checks.
  - [x] Estimate prompt tokens using the existing token estimator for an initial implementation.
  - [x] Add configurable max estimated prompt tokens per request.
  - [x] Add configurable max estimated total tokens per request.
  - [x] Return clean `400` for invalid request size and `429` or `503` for capacity pressure.
- [x] Add optional queueing only if it stays simple.
  - [x] Start with immediate rejection rather than background queues.
  - [x] If queueing is added, make it bounded.
  - [x] Add queue timeout and cancellation behavior.
  - [x] Never allow unbounded request buffering.
- [x] Integrate with middleware or route-level guards.
  - [x] Keep global rate limiting before auth as it is today.
  - [x] Apply admission after auth and request validation.
  - [x] Ensure rejected requests decrement any in-flight counters.
  - [x] Include `Retry-After` when rejection is temporary.
- [x] Add observability.
  - [x] Count admission rejections by reason.
  - [x] Expose current configured limits in docs, not necessarily in API responses.
  - [x] Add logs with request ID, model, key ID, and rejection reason.

Acceptance check:

- [x] Over-capacity traffic receives deterministic clean errors.
- [x] In-flight counters do not leak after rejected, failed, or cancelled requests.
- [x] Tests cover global, pool, provider, and token-budget rejection paths.

## Milestone 5: Prefix Fingerprinting

Goal: identify requests that are likely to share expensive prompt prefixes without logging or storing raw prompt content.

- [x] Define a privacy-preserving prefix fingerprint.
  - [x] Hash normalized leading messages rather than storing prompt text.
  - [x] Include model ID in the fingerprint.
  - [x] Include tool schema or tool-choice shape when present.
  - [x] Include response format when it changes provider behavior.
  - [x] Avoid including volatile user suffixes when possible.
- [x] Add prefix normalization helpers.
  - [x] Support chat completions.
  - [x] Support Responses after conversion to chat request.
  - [x] Treat system prompts and tool schemas as high-value prefix material.
  - [x] Keep normalization deterministic and covered by unit tests.
- [x] Add prefix length and confidence metadata.
  - [x] Estimate prefix character length.
  - [x] Estimate prefix token count with the current heuristic.
  - [x] Mark low-confidence fingerprints when there is no stable prefix.
  - [x] Skip prefix-aware routing for low-confidence requests.
- [x] Add safe observability.
  - [x] Count prefix-fingerprint hits and misses.
  - [x] Expose aggregate prefix-affinity decisions.
  - [x] Do not expose fingerprints in public responses by default.
  - [x] Do not persist raw prompt prefixes.

Acceptance check:

- [x] Two requests with the same system prompt/tool schema and different final user text produce the same stable prefix fingerprint.
- [x] Requests without reusable prefix material are skipped.
- [x] Tests verify prompt text is not present in logs, stats, or cache metadata.

## Milestone 6: Heuristic Prefix-Aware Routing

Goal: route similar-prefix requests to the same replica when load is balanced, while protecting tail latency under imbalance.

- [x] Add a new routing policy, such as `prefix_affinity`.
  - [x] Keep `priority`, `cost`, and `latency` behavior unchanged.
  - [x] Allow prefix-aware routing only for model pools with multiple members.
  - [x] Fall back to latency or priority routing when prefix data is absent.
- [x] Build an in-memory prefix-affinity index.
  - [x] Map prefix fingerprints to recent provider selections.
  - [x] Store bounded entries with TTL.
  - [x] Evict oldest or least-recent entries when capacity is reached.
  - [x] Keep the index process-local in the first implementation and document that limit.
- [x] Score candidates.
  - [x] Prefer a previous provider for the same prefix when load is within a configured threshold.
  - [x] Prefer lower in-flight load when replicas are imbalanced.
  - [x] Penalize open circuits and recently unhealthy providers.
  - [x] Preserve deterministic tie breakers.
- [x] Record routing reasons.
  - [x] `prefix_hit`
  - [x] `prefix_miss`
  - [x] `load_imbalanced`
  - [x] `circuit_open`
  - [x] `fallback`
- [x] Add tests.
  - [x] Same prefix sticks to the same healthy replica.
  - [x] Different prefixes distribute across replicas.
  - [x] Load imbalance overrides affinity.
  - [x] Open circuit overrides affinity.
  - [x] Missing prefix falls back to deterministic policy.

Acceptance check:

- [x] A repeated-prefix workload routes consistently to the same healthy replica under balanced load.
- [x] A hot replica stops receiving affinity traffic when configured imbalance thresholds are exceeded.

## Milestone 7: Precise KV-Cache Awareness Spike

Goal: investigate whether RustyGate should consume runtime-level cache signals from self-hosted inference workers.

- [x] Document supported runtime options.
  - [x] vLLM automatic prefix caching.
  - [x] vLLM KV-cache event streams if available in the target version.
  - [x] Runtime metrics for cache hit rate, queue depth, and KV utilization.
  - [x] OpenAI-compatible local servers that do not expose cache signals.
- [x] Add an experimental feature flag.
  - [x] Name it clearly, such as `kv-cache-events` or `runtime-cache-signals`.
  - [x] Keep it disabled by default.
  - [x] Do not require it for normal gateway use.
- [x] Define a runtime-signal trait.
  - [x] Represent worker identity.
  - [x] Represent queue depth or in-flight load.
  - [x] Represent KV-cache utilization.
  - [x] Represent prefix block residency if precise events are available.
  - [x] Keep the trait mockable for tests.
- [x] Build a mock signal source first.
  - [x] Simulate cache hit fraction per replica.
  - [x] Simulate queue depth.
  - [x] Test routing score behavior without a real GPU runtime.
- [x] Decide whether to implement a real runtime adapter.
  - [x] If the protocol is stable enough, add a narrow adapter.
  - [x] If the protocol is unstable or too heavy, stop at the design spike and document the decision.

Acceptance check:

- [x] The project has a clear go/no-go note for precise KV-cache-aware routing.
- [x] Any experimental code is feature-gated, tested with mocks, and documented as experimental.

## Milestone 8: Streaming, Cancellation, and Backpressure Hardening

Goal: make long-running streaming requests safer under load.

- [x] Add stream idle timeout behavior.
  - [x] Configure maximum time between upstream chunks.
  - [x] Return or emit a clean provider timeout error when exceeded.
  - [x] Track idle timeout metrics.
- [x] Improve cancellation propagation.
  - [x] Detect client disconnects where Axum makes this practical.
  - [x] Stop reading upstream streams after downstream disconnect.
  - [x] Ensure in-flight counters and admission guards release.
- [x] Bound stream memory usage.
  - [x] Keep existing Responses output buffer cap.
  - [x] Add similar reasoning to chat streaming if needed.
  - [x] Avoid collecting full streamed output unless required for usage or response conversion.
- [x] Add stream-specific metrics.
  - [x] TTFT.
  - [x] stream duration.
  - [x] stream completion versus mid-stream failure.
  - [x] downstream cancellation count if observable.
- [x] Add tests.
  - [x] Provider stream stalls.
  - [x] Provider stream fails after first chunk.
  - [x] Client cancellation or simulated dropped stream.
  - [x] Admission counters release after stream termination.

Acceptance check:

- [x] Streaming failures and cancellations do not leak in-flight state.
- [x] Operators can distinguish provider timeout, mid-stream provider failure, and admission rejection.

## Milestone 9: Production Boundary Tightening

Goal: make the internal production story credible without adding heavy infrastructure.

- [x] Add route exposure controls.
  - [x] Allow config to disable OpenAI-shaped placeholder resource routes.
  - [x] Keep `/v1/responses`, `/v1/chat/completions`, `/v1/models`, `/health`, and `/ready` available by default.
  - [x] Document which endpoints are real provider-backed paths.
- [x] Strengthen observability access guidance.
  - [x] Keep role checks for `/stats`, `/stats/providers`, and `/metrics`.
  - [x] Document separate observability keys for staging/prod.
  - [x] Keep network-boundary guidance in operations docs.
- [x] Improve request ID consistency.
  - [x] Consider accepting an incoming request ID header.
  - [x] Propagate the same request ID through auth, rate-limit, admission, and route validation errors.
  - [x] Avoid creating unrelated IDs in middleware for the same inbound request.
- [x] Add key rotation runbook details.
  - [x] Create a new key.
  - [x] Deploy clients with the new key.
  - [x] Revoke the old key.
  - [x] Verify quotas and role scopes.
- [x] Review config profiles.
  - [x] Add v0.4 options to local/staging/prod config examples.
  - [x] Keep risky features disabled by default.
  - [x] Ensure startup validation catches invalid pool/admission/prefix settings.

Acceptance check:

- [x] A production-style internal deployment can expose only real endpoints.
- [x] Auth, rate-limit, admission, and route errors share consistent request correlation.

## Milestone 10: Benchmark Shared-Prefix Workloads

Goal: prove whether v0.4 changes help the intended bottleneck instead of only adding complexity.

- [x] Extend the benchmark harness.
  - [x] Add a shared-prefix workload.
  - [x] Add a no-shared-prefix workload.
  - [x] Add mixed prompt lengths.
  - [x] Add streaming and non-streaming variants if practical.
- [x] Add mock-replica benchmarks.
  - [x] Use deterministic mock replicas to validate routing behavior.
  - [x] Measure gateway overhead added by prefix fingerprinting and admission checks.
  - [x] Confirm no-shared-prefix traffic does not regress substantially.
- [x] Add self-hosted runtime benchmark instructions.
  - [x] Document how to run against local vLLM or another OpenAI-compatible runtime.
  - [x] Keep this optional and separate from default CI.
  - [x] Document required model, hardware, and runtime flags.
- [x] Report the right metrics.
  - [x] Requests per second.
  - [x] p50, p95, and p99 latency.
  - [x] TTFT p50 and p95 for streaming.
  - [x] Admission rejections.
  - [x] Prefix-affinity hit rate.
  - [x] Provider in-flight distribution.
  - [x] If available, runtime prefix/KV-cache hit metrics.
- [x] Update `docs/benchmarks.md`.
  - [x] Separate gateway-overhead results from inference-runtime results.
  - [x] Avoid overclaiming improvements from mock providers.
  - [x] Include raw result file paths and machine profile guidance.

Acceptance check:

- [x] The benchmark can show when prefix-aware routing helps, when it does nothing, and what overhead it adds.
- [x] Results are reproducible enough for portfolio review.

## Milestone 11: Documentation and Release Polish

Goal: make the v0.4 story easy to review.

- [x] Update `docs/architecture.md`.
  - [x] Add the model-pool and admission-control flow.
  - [x] Add a simple diagram for prefix-aware routing.
  - [x] Keep the existing request lifecycle accurate.
- [x] Update `docs/provider-routing.md`.
  - [x] Document model pools.
  - [x] Document prefix-affinity routing.
  - [x] Document load-imbalance fallback.
  - [x] Document deterministic tie breakers.
- [x] Update `docs/failure-handling.md`.
  - [x] Add admission rejection categories.
  - [x] Add stream idle timeout behavior.
  - [x] Add routing fallback behavior under prefix affinity.
- [x] Update `docs/observability.md`.
  - [x] Add new metrics.
  - [x] Add suggested panels for in-flight load, TTFT, prefix-affinity decisions, and admission rejections.
  - [x] Keep dashboard guidance as docs only.
- [x] Add release notes.
  - [x] Create `docs/releases/v0.4.0.md`.
  - [x] Summarize the problem v0.4 addresses.
  - [x] List known limitations honestly.
  - [x] Include migration notes for config changes.
- [x] Update examples.
  - [x] Add a minimal model-pool config.
  - [x] Add a prefix-affinity config.
  - [x] Add an admission-control config.
  - [x] Keep existing examples valid.

Acceptance check:

- [x] A reviewer can understand the v0.4 value proposition from README, roadmap, and release notes.
- [x] Docs do not imply RustyGate controls KV cache unless precise runtime integration exists.

## v0.4 Focused Test Checklist

- [x] Config validation for model pools and pool members.
- [x] Backward compatibility for existing provider-only configs.
- [x] Alias resolution with model pools.
- [x] Candidate selection within a model pool.
- [x] Per-provider in-flight counters release on success.
- [x] Per-provider in-flight counters release on provider failure.
- [x] Per-provider in-flight counters release on validation/admission rejection.
- [x] Streaming TTFT recording.
- [x] Stream idle timeout behavior.
- [x] Admission rejection for global concurrency limit.
- [x] Admission rejection for pool concurrency limit.
- [x] Admission rejection for provider concurrency limit.
- [x] Admission rejection for estimated prompt/token budget.
- [x] Prefix fingerprint stability for repeated stable prefixes.
- [x] Prefix fingerprint skip behavior for low-confidence requests.
- [x] Prefix-affinity routing hit.
- [x] Prefix-affinity routing miss.
- [x] Prefix-affinity overridden by load imbalance.
- [x] Prefix-affinity overridden by circuit state.
- [x] Prefix-affinity metrics and routing-reason counters.
- [x] Route exposure controls for placeholder compatibility endpoints.
- [x] Request ID consistency across middleware errors.
- [x] Benchmark harness smoke test for shared-prefix workload.

## Full Handoff Checklist

Run these before handing off substantive Rust changes:

```sh
cargo fmt
cargo clippy --all-targets --all-features
cargo test
```

For documentation-only changes, at minimum review the edited Markdown and make sure code examples still match the current API.
