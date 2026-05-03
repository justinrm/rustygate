# TODO.md

## Current RustyGate Plan

RustyGate is a Rust inference gateway that exposes a simplified OpenAI-compatible chat endpoint, routes requests to mock and real providers, records latency/token/cost/error metadata, and supports fallback behavior.

The MVP is complete. The milestones below track the path from portfolio project to production-usable gateway. Work top to bottom; each milestone should leave the project deployable and better than before.

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

### Production-Usable Roadmap

The milestones below are the remaining work to make RustyGate production-usable for internal workloads while keeping scope disciplined.

### Milestone 1: Release Baseline

Goal: lock in the current stable baseline and make it easy to compare future hardening work against `v0.1.x`.

- [x] Run final release validation across local and CI paths (`fmt`, `clippy`, `test`).
- [x] Tag `v0.1.0` after validation and create concise release notes.
- [x] Record known limitations in README and `docs/roadmap.md` (streaming, auth, rate limiting, circuit breaker).
- [x] Add a lightweight smoke script for `/health`, `/ready`, one chat request, and `/stats`.

Acceptance check:

- [x] `v0.1.0` is tagged with release notes.
- [x] Smoke script passes on a clean clone.

### Milestone 2: Streaming Chat Completions

Goal: support OpenAI-style streaming responses so interactive clients can use RustyGate in real time.

- [ ] Add `stream` request handling for `POST /v1/chat/completions`.
- [ ] Implement SSE response framing in gateway handlers.
- [ ] Add streaming support for `openai_compatible` providers.
- [ ] Add streaming support for `anthropic` provider mappings.
- [ ] Define streaming error behavior (before first token vs after partial response).
- [ ] Ensure request IDs and provider metadata remain visible in streaming paths.
- [ ] Ensure logs and persistence do not capture prompt content by default in streaming mode.
- [ ] Add integration tests for happy-path streaming and provider stream failures.

Acceptance check:

- [ ] A streaming `curl` request receives incremental chunks and a completion terminator.
- [ ] Streaming tests pass for both provider kinds.

### Milestone 3: Gateway Authentication

Goal: prevent unauthorized use of chat and stats endpoints.

- [ ] Add gateway API key configuration (`gateway.api_key_env` or key list variant).
- [ ] Require `Authorization: Bearer` key for `POST /v1/chat/completions`.
- [ ] Require auth for `/stats` and `/stats/providers`.
- [ ] Keep `/health` and `/ready` unauthenticated.
- [ ] Return `401` with clean error bodies for missing/invalid keys.
- [ ] Ensure keys are never logged.
- [ ] Add integration tests for authenticated and unauthenticated requests.

Acceptance check:

- [ ] Protected routes reject missing/invalid keys with `401`.
- [ ] Valid keys pass without leaking secret material in logs.

### Milestone 4: Rate Limiting and Abuse Protection

Goal: limit abuse and protect upstream providers from burst traffic.

- [ ] Add configurable in-memory rate limiting (global and per API key).
- [ ] Return `429` with `Retry-After` where appropriate.
- [ ] Add request body size limits for chat requests.
- [ ] Add guards for max message count and max content length.
- [ ] Add tests for limit enforcement and reset behavior.

Acceptance check:

- [ ] Repeated requests exceed limits and receive `429`.
- [ ] Oversized payloads are rejected deterministically.

### Milestone 5: Resilience Hardening

Goal: improve failure handling under degraded provider conditions.

- [ ] Add provider circuit breaker state (`closed`, `open`, `half_open`).
- [ ] Track rolling failure counts and open circuits after thresholds.
- [ ] Skip open providers during candidate selection.
- [ ] Add recovery probing behavior for half-open providers.
- [ ] Add per-provider timeout overrides in config.
- [ ] Add retry backoff + jitter for retryable failures.
- [ ] Add unit and integration tests for breaker transitions and fallback under open circuits.

Acceptance check:

- [ ] Persistent provider failures stop receiving immediate traffic.
- [ ] Recovered providers re-enter service via half-open probing.

### Milestone 6: Observability for Operations

Goal: make production incidents diagnosable with standard tooling.

- [ ] Add Prometheus-compatible `/metrics` endpoint.
- [ ] Add request error-rate and in-flight metrics.
- [ ] Add provider-level timeout/rate-limit counters.
- [ ] Add trace/request correlation fields consistently across success and failure paths.
- [ ] Document recommended dashboards and alert thresholds in docs.

Acceptance check:

- [ ] Prometheus can scrape gateway metrics.
- [ ] Alert-worthy signals (error rate, timeout spikes) are exposed.

### Milestone 7: Routing and Compatibility Improvements

Goal: reduce client friction and improve operational routing choices.

- [ ] Add `GET /v1/models` based on configured providers and aliases.
- [ ] Add model alias support in config (`gpt-4o` -> provider-specific model IDs).
- [ ] Add optional cost-aware routing policy.
- [ ] Add optional latency-aware routing policy using recent provider stats.
- [ ] Keep deterministic, testable policy behavior with explicit precedence.
- [ ] Add tests for alias resolution and policy selection.

Acceptance check:

- [ ] Common SDK clients can discover available models via `/v1/models`.
- [ ] Routing policies choose expected providers in deterministic tests.

### Milestone 8: Deployment Hardening

Goal: make runtime behavior predictable in staging and production.

- [ ] Add configuration profiles for local/staging/prod.
- [ ] Add stronger startup config validation (duplicate providers, invalid limits, missing auth config).
- [ ] Add graceful shutdown with in-flight request draining timeout.
- [ ] Add container docs for persistent SQLite volume and environment setup.
- [ ] Add production runbook doc (`docs/operations.md`) with startup, rollback, and incident triage notes.

Acceptance check:

- [ ] Deploy/restart procedures are documented and reproducible.
- [ ] Graceful shutdown behavior is covered by tests or validated scripts.

### Focused Test Coverage Checklist (Next Phase)

- [ ] Streaming completion success and failure paths.
- [ ] Auth enforcement for protected endpoints.
- [ ] Rate limiting (`429`) and retry header behavior.
- [ ] Request body size and input guardrails.
- [ ] Circuit breaker state transitions.
- [ ] Timeout override behavior per provider.
- [ ] `/metrics` scrape and key counters.
- [ ] `/v1/models` and alias resolution behavior.

### Full Handoff Checklist

Run these before handing off substantive Rust changes:

```sh
cargo fmt
cargo clippy --all-targets --all-features
cargo test
```

For documentation-only changes, at minimum review the edited Markdown and make sure code examples still match the current API.
