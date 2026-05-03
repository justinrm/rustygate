# AGENTS.md

RustyGate is a lightweight Rust LLM inference gateway portfolio project. Keep the codebase small, polished, tested, and documentation-first.

## Project Purpose

RustyGate demonstrates practical AI infrastructure patterns: async Rust web services, simplified OpenAI-compatible chat APIs, provider abstraction, request routing, fallback behavior, streaming responses, structured logging, cost estimation, and metrics.

This is a lightweight internal/demo gateway, not a full production inference platform. Do not add heavy infrastructure unless explicitly requested.

## Tech Stack

- Rust stable
- `axum` and `tokio` for the HTTP service
- `serde` and `serde_json` for API payloads
- `thiserror` for domain errors
- `anyhow` only at startup/application boundaries
- `tracing`, `tracing-subscriber`, and `tower-http` for observability
- `toml` and `dotenvy` for local configuration

Add `reqwest`, `sqlx`, or `clap` only when their feature area is actively implemented.

## Scope Boundaries

Core implemented surface:

- `GET /health`
- `GET /ready`
- `POST /v1/chat/completions`
- `POST /v1/responses`
- `GET /v1/models`
- `GET /stats`
- `GET /stats/providers`
- `GET /metrics`

The MVP and post-MVP hardening are complete. A `v0.3` portfolio-hardening roadmap is explicitly open for OpenTelemetry tracing, provider health checks, multi-key auth, bounded and optional Redis-backed rate limiting, tool calling, opt-in response caching, semantic caching behind a feature flag, and reproducible benchmarks. Keep this work small, documented, and testable.

Do not add Kubernetes manifests, multi-user billing, a web dashboard, or production policy engines without explicit approval.

## Coding Standards

- Prefer readable, idiomatic async Rust over clever abstractions.
- Keep modules small and named after their responsibility.
- Keep provider selection out of HTTP route handlers.
- Use `Result` types for application logic instead of panics.
- Use structured error enums and map them to clean JSON responses.
- Document structs and enums when they represent public project concepts.

## Testing Expectations

- Run focused unit tests for routing, config parsing, metrics, token/cost estimation, and fallback decisions.
- Run integration tests for HTTP behavior.
- Never require real external provider APIs in tests.
- Cover failure cases, not only happy paths.
- Keep example JSON valid and synchronized with API models.

## Security And Logging

- Never commit real API keys.
- Never log API keys, Authorization headers, or prompt content by default.
- Redact provider raw errors before returning client responses.
- Include request IDs in logs and client-facing error bodies when available.
- Keep `.env` ignored and commit only `.env.example`.

## Documentation Expectations

- Keep the README practical and portfolio-friendly.
- Clearly label MVP features versus stretch goals.
- Document tradeoffs honestly.
- Update docs and examples when API shapes or config fields change.

## Command Checklist

Run these before handing off substantive Rust changes:

```sh
cargo fmt
cargo clippy --all-targets --all-features
cargo test
```
