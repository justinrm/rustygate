# Roadmap

RustyGate should grow in small, reviewable steps.

## Current Status

RustyGate is past the original MVP and now sits at a lightweight internal/demo gateway baseline. The current feature set includes mock and real providers, non-streaming and SSE streaming chat completions, gateway bearer auth, in-memory rate limiting, retries, fallback, circuit breakers, Prometheus-compatible metrics, model discovery, optional SQLite persistence, Docker, CI, deployment profiles, and an operations runbook.

The project is now entering a focused `v0.2` compatibility scope. The goal is a Responses-first path toward OpenAI API compatibility while preserving the small, testable gateway shape.

## MVP

- Axum service with `/health` and `/ready`
- Simplified chat completion request and response models
- Mock provider with deterministic responses
- Basic request validation
- Priority routing by exact model match
- Same-provider retries and fallback for retryable provider failures
- In-memory metrics and stats endpoints with aggregate and provider-level latency
- Structured request metadata logs with prompt redaction by default
- Optional SQLite request log and provider-attempt persistence
- OpenAI-compatible provider behind the provider trait
- Anthropic provider behind the provider trait
- Gateway-level outbound request timeout handling
- Focused unit and integration tests
- Practical README and examples
- GitHub Actions CI for `fmt`, `clippy`, and tests
- Minimal Dockerfile for local portfolio demos
- Deployment profiles, startup config validation, graceful shutdown, and operations docs

## Post-MVP Hardening Completed

- SSE streaming for mock, OpenAI-compatible, and Anthropic providers
- Gateway bearer auth for chat, stats, metrics, and model discovery
- Configurable in-memory global and per-key rate limiting
- Chat request body, message count, and message content limits
- Provider circuit breakers with half-open recovery probes
- Configurable retry backoff and jitter
- Prometheus-compatible `/metrics`
- `GET /v1/models` with configured aliases
- Optional cost-aware and latency-aware routing policies

## Remaining Lightweight Gaps

- Provider health checks
- Multi-key auth management and key rotation workflow
- Distributed/shared rate limiting beyond a single process
- SQLite retention and cleanup controls
- OpenTelemetry traces
- Optional default model configuration

## v0.2 Responses-First OpenAI Compatibility

Compatibility work should land in small, reviewable slices with endpoint-level tests and documentation updates. The initial target is the modern `/v1/responses` API, followed by legacy chat alignment and high-impact endpoint families used by common SDK clients.

Acceptance criteria:

- `/v1/responses` supports non-streaming and SSE streaming requests through the gateway routing/fallback path.
- Legacy `/v1/chat/completions` remains available and continues to work for existing clients.
- OpenAI-shaped endpoints exist for embeddings, moderations, images, audio transcription/translation, files, batches, fine-tuning jobs, and realtime session creation.
- Client-facing errors, stream events, model objects, and generated IDs follow OpenAI-compatible shapes wherever practical.
- Gateway-specific metadata remains available through existing operational endpoints rather than becoming required in OpenAI API responses.
- Compatibility docs, examples, smoke checks, and integration tests stay synchronized with implemented behavior.

## Do Not Build Yet

- Kubernetes manifests
- Multi-user billing
- Complex authentication
- Redis
- Web dashboard
- Semantic caching
- Production-grade policy engine
