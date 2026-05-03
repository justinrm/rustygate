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
- `GET /v1/models`
- `GET /stats`
- `GET /stats/providers`
- `GET /metrics`

The MVP and post-MVP hardening are complete. Keep future work focused on validation, bugs, docs, and small hardening unless a new roadmap is explicitly opened. Do not add Kubernetes manifests, multi-user billing, complex authentication, Redis, semantic caching, a web dashboard, production policy engines, or full OpenAI API compatibility without explicit approval.

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

## Cursor Cloud specific instructions

### Running the service

1. Ensure `.env` exists (copy from `.env.example` if missing — it sets `RUSTYGATE_GATEWAY_API_KEY=local-dev-gateway-key` and uses mock providers).
2. Start with `cargo run`. The server listens on `127.0.0.1:8080`.
3. No external services are required — the default `config/gateway.local.toml` uses only in-process mock providers.

### Testing endpoints

All protected endpoints require `Authorization: Bearer local-dev-gateway-key` (the value from `.env`).

```sh
curl http://127.0.0.1:8080/health
curl -H "Authorization: Bearer local-dev-gateway-key" http://127.0.0.1:8080/v1/models
curl -X POST http://127.0.0.1:8080/v1/chat/completions \
  -H "Authorization: Bearer local-dev-gateway-key" \
  -H "Content-Type: application/json" \
  -d '{"model":"mock-fast","messages":[{"role":"user","content":"test"}]}'
```

### Notes

- The `rust-toolchain.toml` pins `channel = "stable"`; rustup auto-installs it.
- SQLite storage is disabled by default (`storage.enabled = false` in local config). No DB setup needed.
- Real provider API keys (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`) are only needed for staging/prod config profiles — never for local development or tests.
- All 89 tests (unit + integration) run without network access or external dependencies.
