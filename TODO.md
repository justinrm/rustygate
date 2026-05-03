# TODO.md

## Current RustyGate Plan

RustyGate is a lightweight Rust inference gateway that exposes a simplified OpenAI-compatible chat endpoint, routes requests to mock and real providers, supports non-streaming and SSE streaming responses, records latency/token/cost/error metadata, and handles retries, fallback, and circuit breaker recovery.

The MVP, `v0.2` Responses compatibility work, and the `v0.3` portfolio-hardening follow-ups are complete for a lightweight internal/demo gateway. Keep future work small, documented, and clearly labeled as demo or production-shaped.

### v0.3 Portfolio Hardening (Complete)

- [x] OpenTelemetry tracing with provider-attempt spans and a demo bundle.
- [x] Provider health probes feeding `/ready`.
- [x] Multi-key auth with hashed SQLite storage, roles, quotas, and an admin CLI.
- [x] Bounded local rate-limit state and optional Redis-backed rate limiting.
- [x] Tool/function calling support across mock, OpenAI-compatible, and Anthropic provider paths.
- [x] Opt-in exact-match response caching with hit/miss metrics.
- [x] Experimental semantic-cache feature gate.
- [x] Benchmark harness and benchmark documentation.
- [x] Integration tests for auth, caching, health, tools, Redis feature wiring, and semantic cache behavior.

### How To Use This Checklist

Work from top to bottom. Each milestone should leave the project runnable and better than before.

For each implementation slice:

1. Make the smallest code change that completes the next unchecked task.
2. Add or update focused tests for the behavior.
3. Run `cargo fmt`.
4. Run the most relevant focused tests.
5. Run `cargo clippy --all-targets --all-features` and `cargo test` before a larger handoff.
6. Update examples or docs if the API shape, config shape, or user-facing behavior changed.

### MVP (Complete)

- [x] `GET /health`, `GET /ready`, `POST /v1/chat/completions`, `GET /stats`, `GET /stats/providers`
- [x] Mock providers with deterministic responses
- [x] Real outbound providers: `openai_compatible` and `anthropic`
- [x] Priority routing by exact model match with fallback across eligible providers
- [x] In-memory and optional SQLite metrics and request logging
- [x] Structured JSON errors with request IDs
- [x] Prompt redaction by default
- [x] GitHub Actions CI, Dockerfile, example configs

### Lightweight Production-Usable Roadmap (Complete)

The milestones below document the completed path from MVP to a production-style internal gateway while keeping scope disciplined. Remaining production gaps are intentionally outside this checklist: multi-key auth and rotation, distributed/shared rate limiting, provider health checks in readiness, retention controls for persisted logs, and OpenTelemetry tracing.

### Milestone 1: Release Baseline

Goal: lock in the current stable baseline and make it easy to compare future hardening work against `v0.1.x`.

- [x] Run final release validation across local and CI paths (`fmt`, `clippy`, `test`).
- [x] Tag `v0.1.0` after validation and create concise release notes.
- [x] Record known limitations in README and `docs/roadmap.md` (multi-key auth, distributed rate limiting, provider health checks, retention controls).
- [x] Add a lightweight smoke script for `/health`, `/ready`, one chat request, and `/stats`.

Acceptance check:

- [x] `v0.1.0` is tagged with release notes.
- [x] Smoke script passes on a clean clone.

### Milestone 2: Streaming Chat Completions

Goal: support OpenAI-style streaming responses so interactive clients can use RustyGate in real time.

- [x] Add `stream` request handling for `POST /v1/chat/completions`.
- [x] Implement SSE response framing in gateway handlers.
- [x] Add streaming support for `openai_compatible` providers.
- [x] Add streaming support for `anthropic` provider mappings.
- [x] Define streaming error behavior (before first token vs after partial response).
- [x] Ensure request IDs and provider metadata remain visible in streaming paths.
- [x] Ensure logs and persistence do not capture prompt content by default in streaming mode.
- [x] Add integration tests for happy-path streaming and provider stream failures.

Acceptance check:

- [x] A streaming `curl` request receives incremental chunks and a completion terminator.
- [x] Streaming tests pass for both provider kinds.

### Milestone 3: Gateway Authentication

Goal: prevent unauthorized use of chat and stats endpoints.

- [x] Add gateway API key configuration (`gateway.api_key_env` or key list variant).
- [x] Require `Authorization: Bearer` key for `POST /v1/chat/completions`.
- [x] Require auth for `/stats` and `/stats/providers`.
- [x] Keep `/health` and `/ready` unauthenticated.
- [x] Return `401` with clean error bodies for missing/invalid keys.
- [x] Ensure keys are never logged.
- [x] Add integration tests for authenticated and unauthenticated requests.

Acceptance check:

- [x] Protected routes reject missing/invalid keys with `401`.
- [x] Valid keys pass without leaking secret material in logs.

### Milestone 4: Rate Limiting and Abuse Protection

Goal: limit abuse and protect upstream providers from burst traffic.

- [x] Add configurable in-memory rate limiting (global and per API key).
- [x] Return `429` with `Retry-After` where appropriate.
- [x] Add request body size limits for chat requests.
- [x] Add guards for max message count and max content length.
- [x] Add tests for limit enforcement and reset behavior.

Acceptance check:

- [x] Repeated requests exceed limits and receive `429`.
- [x] Oversized payloads are rejected deterministically.

### Milestone 5: Resilience Hardening

Goal: improve failure handling under degraded provider conditions.

- [x] Add provider circuit breaker state (`closed`, `open`, `half_open`).
- [x] Track rolling failure counts and open circuits after thresholds.
- [x] Skip open providers during candidate selection.
- [x] Add recovery probing behavior for half-open providers.
- [x] Add per-provider timeout overrides in config.
- [x] Add retry backoff + jitter for retryable failures.
- [x] Add unit and integration tests for breaker transitions and fallback under open circuits.

Acceptance check:

- [x] Persistent provider failures stop receiving immediate traffic.
- [x] Recovered providers re-enter service via half-open probing.

### Milestone 6: Observability for Operations

Goal: make production incidents diagnosable with standard tooling.

- [x] Add Prometheus-compatible `/metrics` endpoint.
- [x] Add request error-rate and in-flight metrics.
- [x] Add provider-level timeout/rate-limit counters.
- [x] Add trace/request correlation fields consistently across success and failure paths.
- [x] Document recommended dashboards and alert thresholds in docs.

Acceptance check:

- [x] Prometheus can scrape gateway metrics.
- [x] Alert-worthy signals (error rate, timeout spikes) are exposed.

### Milestone 7: Routing and Compatibility Improvements

Goal: reduce client friction and improve operational routing choices.

- [x] Add `GET /v1/models` based on configured providers and aliases.
- [x] Add model alias support in config (`gpt-4o` -> provider-specific model IDs).
- [x] Add optional cost-aware routing policy.
- [x] Add optional latency-aware routing policy using recent provider stats.
- [x] Keep deterministic, testable policy behavior with explicit precedence.
- [x] Add tests for alias resolution and policy selection.

Acceptance check:

- [x] Common SDK clients can discover available models via `/v1/models`.
- [x] Routing policies choose expected providers in deterministic tests.

### Milestone 8: Deployment Hardening

Goal: make runtime behavior predictable in staging and production.

- [x] Add configuration profiles for local/staging/prod.
- [x] Add stronger startup config validation (duplicate providers, invalid limits, missing auth config).
- [x] Add graceful shutdown with in-flight request draining timeout.
- [x] Add container docs for persistent SQLite volume and environment setup.
- [x] Add production runbook doc (`docs/operations.md`) with startup, rollback, and incident triage notes.

Acceptance check:

- [x] Deploy/restart procedures are documented and reproducible.
- [x] Graceful shutdown behavior is covered by tests or validated scripts.

### Focused Test Coverage Checklist (Complete)

- [x] Streaming completion success and failure paths.
- [x] Auth enforcement for protected endpoints.
- [x] Rate limiting (`429`) and retry header behavior.
- [x] Request body size and input guardrails.
- [x] Circuit breaker state transitions.
- [x] Timeout override behavior per provider.
- [x] `/metrics` scrape and key counters.
- [x] `/v1/models` and alias resolution behavior.

### v0.2 Responses-First OpenAI Compatibility

Goal: expose a practical OpenAI-compatible API surface, starting with `/v1/responses`, without turning RustyGate into a heavy production inference platform.

- [x] Open v0.2 scope and document endpoint compatibility acceptance criteria.
- [x] Add shared OpenAI compatibility helpers for public IDs and timestamps.
- [x] Add `POST /v1/responses` with non-streaming and SSE streaming response events.
- [x] Keep `/v1/chat/completions` available while sharing routing/fallback behavior with Responses.
- [x] Add OpenAI-shaped embeddings, moderations, audio, and image endpoints.
- [x] Add lightweight files, batches, fine-tuning, and realtime session endpoints.
- [x] Add integration tests and smoke checks for the compatibility surface.
- [x] Update README and architecture/failure-handling docs for the new surface.

Acceptance check:

- [x] Common OpenAI SDK-style routes return OpenAI-shaped JSON or SSE payloads.
- [x] Existing gateway observability and protected-route behavior remain intact.

### Full Handoff Checklist

Run these before handing off substantive Rust changes:

```sh
cargo fmt
cargo clippy --all-targets --all-features
cargo test
```

For documentation-only changes, at minimum review the edited Markdown and make sure code examples still match the current API.
