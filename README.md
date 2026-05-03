# RustyGate: LLM Inference Gateway Lite

RustyGate is a lightweight Rust inference gateway portfolio project. It demonstrates async service boundaries, OpenAI-compatible API shapes, provider abstraction, and observability basics without pretending to be production-ready.

## Why This Project Exists

Modern AI apps often need a stable boundary between product code and model providers. RustyGate explores that boundary in a compact, testable Rust codebase.

## Current Status

Implemented now:

- `GET /health` and `GET /ready`
- `POST /v1/responses` with non-streaming and SSE streaming responses
- `POST /v1/chat/completions` with request validation and deterministic mock providers
- OpenAI-shaped compatibility endpoints for embeddings, moderations, images, audio, files, batches, fine-tuning jobs, and realtime session creation
- Real outbound provider support for `openai_compatible` and `anthropic` provider kinds
- Provider selection by model match and priority
- Fallback across eligible providers for retryable provider errors
- Same-provider retries with bounded backoff and jitter
- Provider circuit breaker states with half-open recovery probes
- In-memory `/stats` and `/stats/providers` metrics for requests, attempts, fallback attempts, latency, tokens, and estimated cost
- Prometheus-compatible `/metrics` for operational scraping
- `GET /v1/models` model discovery with configured aliases
- Optional priority, cost-aware, and latency-aware routing policies
- Structured JSON errors with request IDs
- Startup, HTTP tracing, and structured request metadata logs
- Optional SQLite persistence for request logs and provider attempts
- OpenTelemetry trace export with provider-attempt spans and trace-context propagation
- Provider health probes feeding `/ready?detail=true`
- SQLite-backed multi-key auth with hashed keys, roles, quotas, and `rustygate_admin`
- Bounded local rate-limit state plus optional Redis-backed rate limiting behind `redis-backend`
- OpenAI-compatible tool/function calling for mock, OpenAI-compatible, and Anthropic paths
- Opt-in exact-match response caching with cache hit/miss metrics
- Experimental semantic cache primitives behind `semantic-cache`
- Reproducible benchmark harness under `benchmarks/`
- Config loading from TOML (`RUSTYGATE_CONFIG` override supported)

Release notes: `docs/releases/v0.2.0.md`

Operations runbook: `docs/operations.md`

OpenAI compatibility matrix: `docs/openai-compatibility.md`

## Non-Goals

No Kubernetes manifests, multi-user billing, web dashboard, or production policy engines. Redis and semantic caching exist only as optional portfolio-hardening features and remain off by default.

## Known Limitations

- Circuit breaker failure tracking is consecutive-failure based and in-memory only.
- The semantic cache is experimental and should be enabled only for demos or controlled tests.
- Benchmarks use mock upstream behavior and measure gateway overhead, not real LLM latency.

## Quickstart

Prerequisites:

- Rust stable toolchain (`rustup` + `cargo`)

Run locally:

```sh
# Optional: point to a deployment profile.
export RUSTYGATE_CONFIG=config/gateway.local.toml
export RUSTYGATE_GATEWAY_API_KEY=local-dev-gateway-key

cargo run
```

Default config path is `config/gateway.example.toml` when `RUSTYGATE_CONFIG` is unset.
Use `config/gateway.local.toml`, `config/gateway.staging.toml`, or `config/gateway.prod.toml` when you want an explicit runtime profile.

Alternate example profile:

```sh
export RUSTYGATE_CONFIG=examples/mock-config.toml
cargo run
```

Real-provider profile (requires API keys in environment variables):

```sh
export RUSTYGATE_CONFIG=config/gateway.live.example.toml
export RUSTYGATE_GATEWAY_API_KEY=local-dev-gateway-key
export OPENAI_API_KEY=...
export ANTHROPIC_API_KEY=...
cargo run
```

Manage SQLite-backed API keys when `[storage].enabled = true`:

```sh
cargo run --bin rustygate_admin -- keys create --label local-dev --role admin
cargo run --bin rustygate_admin -- keys list
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
docker run --rm -p 8080:8080 \
  -e RUSTYGATE_GATEWAY_API_KEY=local-dev-gateway-key \
  -e RUSTYGATE_CONFIG=config/gateway.local.toml \
  rustygate:local
```

The container uses `config/gateway.example.toml` by default via `RUSTYGATE_CONFIG`.
For SQLite persistence, mount a writable volume and use a profile with `[storage].enabled = true`:

```sh
docker run --rm -p 8080:8080 \
  -e RUSTYGATE_CONFIG=config/gateway.staging.toml \
  -e RUSTYGATE_GATEWAY_API_KEY=change-me \
  -e OPENAI_API_KEY=... \
  -v rustygate-data:/data \
  rustygate:local
```

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
  -H "authorization: Bearer ${RUSTYGATE_GATEWAY_API_KEY}" \
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

Streaming response with SSE (`stream: true`):

```sh
curl -N http://127.0.0.1:8080/v1/chat/completions \
  -H 'content-type: application/json' \
  -H "authorization: Bearer ${RUSTYGATE_GATEWAY_API_KEY}" \
  -d '{
    "model": "mock-fast-v1",
    "stream": true,
    "messages": [
      {"role": "user", "content": "Count from one to three."}
    ]
  }'
```

Expected stream shape:

```text
data: {"id":"...","object":"chat.completion.chunk","model":"mock-fast-v1","provider":"mock-fast",...}

data: {"id":"...","object":"chat.completion.chunk","model":"mock-fast-v1","provider":"mock-fast",...}

data: [DONE]
```

Example response shape:

```json
{
  "id": "chatcmpl-3f7dc5a26dc20f888f55b664e2f8f66c",
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

## Example Responses Request

`POST /v1/responses` is the canonical OpenAI-compatible surface for new clients:

```sh
curl -sS http://127.0.0.1:8080/v1/responses \
  -H 'content-type: application/json' \
  -H "authorization: Bearer ${RUSTYGATE_GATEWAY_API_KEY}" \
  -d '{
    "model": "mock-fast-v1",
    "input": "Explain what an inference gateway does in two sentences.",
    "temperature": 0.2,
    "max_output_tokens": 128
  }'
```

Streaming Responses use SSE event names such as `response.output_text.delta` and `response.completed`:

```sh
curl -N http://127.0.0.1:8080/v1/responses \
  -H 'content-type: application/json' \
  -H "authorization: Bearer ${RUSTYGATE_GATEWAY_API_KEY}" \
  -d '{
    "model": "mock-fast-v1",
    "stream": true,
    "input": "Count from one to three."
  }'
```

Stats endpoints expose aggregate request and provider-attempt metrics:

```sh
curl -H "authorization: Bearer ${RUSTYGATE_GATEWAY_API_KEY}" http://127.0.0.1:8080/stats
curl -H "authorization: Bearer ${RUSTYGATE_GATEWAY_API_KEY}" http://127.0.0.1:8080/stats/providers
curl -H "authorization: Bearer ${RUSTYGATE_GATEWAY_API_KEY}" http://127.0.0.1:8080/metrics
```

`/stats` reports request totals, success/failure counts, in-flight requests, categorized request errors, latency average and p95, prompt/completion token totals, and input/output estimated cost totals. `/stats/providers` reports provider attempt counts, success counts, error counts, provider error categories, fallback attempt counts, and provider latency average and p95. `/metrics` exposes the same operational signals in Prometheus text format. Stats do not include prompt text or secrets.

Model discovery:

```sh
curl -H "authorization: Bearer ${RUSTYGATE_GATEWAY_API_KEY}" http://127.0.0.1:8080/v1/models
```

## Configuration

Start with `config/gateway.local.toml` for local mock-provider development, or `config/gateway.example.toml` for the compact default example.

Additional ready-to-run example config with two mock providers: `examples/mock-config.toml`.

Real-provider example config: `config/gateway.live.example.toml`.

Deployment profiles:

- `config/gateway.local.toml` uses mock providers and binds to `127.0.0.1`.
- `config/gateway.staging.toml` binds to `0.0.0.0`, enables SQLite at `/data/rustygate-staging.db`, and uses a real OpenAI-compatible primary with a mock fallback.
- `config/gateway.prod.toml` binds to `0.0.0.0`, enables SQLite at `/data/rustygate.db`, and expects gateway/provider keys from environment variables.

- `model` is required for chat requests in the MVP; routing uses exact model matches.
- `gateway.enable_request_logging` emits structured metadata for chat requests. Set `gateway.log_prompt_content = true` only for local development when you intentionally want prompt messages stored in request logs.
- `gateway.api_key_env` points to the env var that contains the required gateway bearer token for protected routes.
- `gateway.rate_limit` controls in-memory token-bucket limits for global traffic and per-key traffic.
- `gateway.request_limits` controls max chat payload size, max message count, and max message content length.
- `gateway.model_aliases` maps public model IDs to provider-specific configured model IDs.
- `gateway.routing_policy` can be `priority`, `cost`, or `latency`; default configs use `priority`.
- `[storage]` controls optional SQLite request-log persistence. It is disabled by default; when enabled, request logs and provider attempts are persisted and stats are read from SQLite.
- `gateway.default_timeout_ms` applies to outbound HTTP calls for real providers, and provider `timeout_ms` can override it.
- `gateway.max_retries` plus `gateway.retry.*` control same-provider retries with backoff and jitter; provider-level retry fields can override defaults.
- `gateway.circuit_breaker.*` controls circuit state transitions; provider-level circuit-breaker fields can override defaults.
- `server.shutdown_grace_period_ms` controls how long shutdown waits for in-flight requests to drain after Ctrl-C or SIGTERM.
- Mock providers require no real API keys or outbound network access.
- Mock providers fail only when `failure_rate = 1.0`; fractional failure rates and `base_latency_ms` are reserved for future simulation work.
- `.env` stays ignored; keep only `.env.example` in version control.

### Real Providers

RustyGate currently supports:

- `kind = "openai_compatible"` for OpenAI-style `/chat/completions` APIs
- `kind = "anthropic"` for Anthropic Messages API

Provider credentials are configured by env var name (`api_key_env`) and read at startup. Keys are never logged.

## Operational Notes

RustyGate is intended for local development and portfolio review. `POST /v1/chat/completions`, `/stats`, and `/stats/providers` require a bearer token; keep `RUSTYGATE_GATEWAY_API_KEY` in local env configuration and never commit it.

If a staging or production-style profile binds to `0.0.0.0`, put RustyGate behind HTTPS termination or an internal-only network boundary. Do not expose the plain HTTP listener directly to the public internet because Bearer tokens and prompts would otherwise travel without transport encryption.

The same gateway Bearer token protects inference and aggregate stats/metrics in this lightweight demo. Treat `/stats`, `/stats/providers`, and `/metrics` as internal endpoints and restrict network access when sharing an inference key.

Chat responses and chat error responses include a gateway request ID for debugging (`id` on success responses and `error.request_id` on error responses). Structured request logs include the same gateway request ID plus route, model, provider, latency, token estimates, cost estimate, fallback counts, and classified errors.

In-memory metrics remain the default lightweight path. SQLite persistence is optional and intended for local portfolio demos that need request logs to survive a restart.

Compatibility endpoints that return file, batch, fine-tuning, image, or realtime session shapes are stubs for SDK compatibility. Generated realtime `client_secret` values are synthetic RustyGate IDs, not provider-issued credentials.

## Testing

```sh
cargo fmt
cargo clippy --all-targets --all-features
cargo test
```

Optional official SDK smoke check:

```sh
python scripts/openai_sdk_smoke.py
```

## License

This project is licensed under the MIT License. See `LICENSE`.
