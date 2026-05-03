# RustyGate: LLM Inference Gateway Lite

RustyGate is a lightweight Rust inference gateway portfolio project. It demonstrates async service boundaries, simplified OpenAI-compatible chat APIs, provider abstraction, and observability basics without pretending to be production-ready.

## Why This Project Exists

Modern AI apps often need a stable boundary between product code and model providers. RustyGate explores that boundary in a compact, testable Rust codebase.

## Current Status

Implemented now:

- `GET /health` and `GET /ready`
- `POST /v1/chat/completions` with request validation and deterministic mock providers
- Real outbound provider support for `openai_compatible` and `anthropic` provider kinds
- Provider selection by model match and priority
- Fallback across eligible providers for retryable provider errors
- In-memory `/stats` and `/stats/providers` metrics for requests, attempts, fallback attempts, latency, tokens, and estimated cost
- Structured JSON errors with request IDs
- Startup, HTTP tracing, and structured request metadata logs
- Optional SQLite persistence for request logs and provider attempts
- Config loading from TOML (`RUSTYGATE_CONFIG` override supported)

Release notes: `docs/releases/v0.1.0.md`

## Non-Goals (MVP)

No streaming responses, Kubernetes manifests, multi-user billing, complex auth, Redis, web dashboard, semantic caching, production policy engines, or full OpenAI API compatibility.

## Known Limitations (v0.1.0)

- No streaming chat completions (`stream: true` is not implemented yet)
- No gateway authentication on chat and stats endpoints
- No gateway rate limiting or abuse throttling
- No provider circuit breaker state management

## Quickstart

Prerequisites:

- Rust stable toolchain (`rustup` + `cargo`)

Run locally:

```sh
# Optional: point to a custom config file.
export RUSTYGATE_CONFIG=config/gateway.example.toml

cargo run
```

Default config path is `config/gateway.example.toml` when `RUSTYGATE_CONFIG` is unset.

Alternate example profile:

```sh
export RUSTYGATE_CONFIG=examples/mock-config.toml
cargo run
```

Real-provider profile (requires API keys in environment variables):

```sh
export RUSTYGATE_CONFIG=config/gateway.live.example.toml
export OPENAI_API_KEY=...
export ANTHROPIC_API_KEY=...
cargo run
```

Verify endpoints:

```sh
curl http://127.0.0.1:8080/health
curl http://127.0.0.1:8080/ready
```

Run the lightweight smoke check (with service already running):

```sh
./scripts/smoke.sh
```

Optional custom base URL:

```sh
BASE_URL=http://127.0.0.1:8080 ./scripts/smoke.sh
```

Expected startup log fields include the bind address and loaded provider names. Example shape:

```text
... INFO starting RustyGate addr=127.0.0.1:8080 provider_count=2 providers=["mock-fast","mock-reliable"]
```

Startup and request logging intentionally avoid API keys, Authorization headers, and prompt content by default.

## Optional Docker Run (Local Demo)

Docker is provided as a convenience for local portfolio demos.

```sh
docker build -t rustygate:local .
docker run --rm -p 8080:8080 rustygate:local
```

The container uses `config/gateway.example.toml` by default via `RUSTYGATE_CONFIG`.

## Example Chat Request

Canonical request payload file: `examples/chat-request.json`

```json
{
  "model": "mock-fast-v1",
  "messages": [
    {
      "role": "system",
      "content": "You are a concise assistant."
    },
    {
      "role": "user",
      "content": "Explain what an inference gateway does in two sentences."
    }
  ],
  "temperature": 0.2,
  "max_tokens": 128
}
```

Run the same request with `curl`:

```sh
curl -sS http://127.0.0.1:8080/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{
    "model": "mock-fast-v1",
    "messages": [
      {"role": "system", "content": "You are a concise assistant."},
      {"role": "user", "content": "Explain what an inference gateway does in two sentences."}
    ],
    "temperature": 0.2,
    "max_tokens": 128
  }'
```

Example response shape:

```json
{
  "id": "3f7dc5a2-6dc2-0f88-8f55-b664e2f8f66c",
  "object": "chat.completion",
  "created": 1700000000,
  "model": "mock-fast-v1",
  "provider": "mock-fast",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "Deterministic mock response from mock-fast."
      },
      "finish_reason": "stop"
    }
  ],
  "usage": {
    "prompt_tokens": 15,
    "completion_tokens": 5,
    "total_tokens": 20
  }
}
```

Estimated token cost is intentionally tracked in internal aggregate metrics and exposed via `/stats`, not added to the chat completion response payload.

Stats endpoints expose aggregate request and provider-attempt metrics:

```sh
curl http://127.0.0.1:8080/stats
curl http://127.0.0.1:8080/stats/providers
```

`/stats` reports request totals, success/failure counts, latency average and p95, prompt/completion token totals, and input/output estimated cost totals. `/stats/providers` reports provider attempt counts, success counts, error counts, fallback attempt counts, and provider latency average and p95. Stats do not include prompt text or secrets.

## Configuration

Start with `config/gateway.example.toml`.

Additional ready-to-run example config with two mock providers: `examples/mock-config.toml`.

Real-provider example config: `config/gateway.live.example.toml`.

- `model` is required for chat requests in the MVP; routing uses exact model matches.
- `gateway.enable_request_logging` emits structured metadata for chat requests. Set `gateway.log_prompt_content = true` only for local development when you intentionally want prompt messages stored in request logs.
- `[storage]` controls optional SQLite request-log persistence. It is disabled by default; when enabled, request logs and provider attempts are persisted and stats are read from SQLite.
- `gateway.default_timeout_ms` applies to outbound HTTP calls for real providers.
- Mock providers require no real API keys or outbound network access.
- Mock providers fail only when `failure_rate = 1.0`; fractional failure rates and `base_latency_ms` are reserved for future simulation work.
- `.env` stays ignored; keep only `.env.example` in version control.

### Real Providers

RustyGate currently supports:

- `kind = "openai_compatible"` for OpenAI-style `/chat/completions` APIs
- `kind = "anthropic"` for Anthropic Messages API

Provider credentials are configured by env var name (`api_key_env`) and read at startup. Keys are never logged.

## Operational Notes

RustyGate is intended for local development and portfolio review. The chat and stats endpoints are not authenticated; keep the service bound to loopback or behind private-network controls if you run it outside your machine.

Chat responses and chat error responses include a gateway request ID for debugging (`id` on success responses and `error.request_id` on error responses). Structured request logs include the same gateway request ID plus route, model, provider, latency, token estimates, cost estimate, fallback counts, and classified errors.

In-memory metrics remain the default lightweight path. SQLite persistence is optional and intended for local portfolio demos that need request logs to survive a restart.

## Testing

```sh
cargo fmt
cargo clippy --all-targets --all-features
cargo test
```
