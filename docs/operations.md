# Operations Runbook

RustyGate is still a compact portfolio gateway, but this runbook describes a repeatable internal staging or demo deployment.

## Runtime Profiles

Select a profile with `RUSTYGATE_CONFIG`:

- `config/gateway.local.toml` for local mock-provider development.
- `config/gateway.staging.toml` for containerized staging checks with SQLite persistence under `/data`.
- `config/gateway.prod.toml` for production-style internal deployments with real provider credentials.

Required environment variables:

```sh
export RUSTYGATE_CONFIG=config/gateway.staging.toml
export RUSTYGATE_GATEWAY_API_KEY=change-me
export OPENAI_API_KEY=...
export ANTHROPIC_API_KEY=...
```

Startup validates config shape before the service binds. Invalid provider names, bad limits, missing real-provider `api_key_env` settings, or aliases that target unknown provider models fail fast.

Production-style profiles can set `gateway.route_exposure.placeholder_compat_routes = false` to expose only the real inference/model endpoints plus health checks. Leave it enabled for local SDK compatibility demos that need placeholder resources such as embeddings, files, batches, fine-tuning jobs, or realtime sessions.

## Startup

Local:

```sh
export RUSTYGATE_CONFIG=config/gateway.local.toml
export RUSTYGATE_GATEWAY_API_KEY=local-dev-gateway-key
cargo run
```

Container:

```sh
docker build -t rustygate:local .
docker run --rm -p 8080:8080 \
  -e RUSTYGATE_CONFIG=config/gateway.staging.toml \
  -e RUSTYGATE_GATEWAY_API_KEY=change-me \
  -e OPENAI_API_KEY=... \
  -v rustygate-data:/data \
  rustygate:local
```

Use a persistent volume whenever `[storage].enabled = true`; the staging and prod profiles store SQLite files below `/data`.

Staging and prod profiles bind to `0.0.0.0` for container deployments. Put that listener behind HTTPS termination or an internal-only network boundary; do not expose RustyGate's plain HTTP port directly to the public internet.

## Health Checks

Unauthenticated checks:

```sh
curl -fsS http://127.0.0.1:8080/health
curl -fsS http://127.0.0.1:8080/ready
```

Authenticated smoke check:

```sh
RUSTYGATE_GATEWAY_API_KEY=change-me ./scripts/smoke.sh
```

Use `BASE_URL` for non-local targets:

```sh
BASE_URL=https://rustygate.example.internal \
RUSTYGATE_GATEWAY_API_KEY=change-me \
./scripts/smoke.sh
```

## Shutdown And Restart

RustyGate handles Ctrl-C and SIGTERM. After a shutdown signal, it stops accepting new work and allows in-flight requests to drain for `server.shutdown_grace_period_ms`.

Recommended restart flow:

1. Start the replacement instance with the intended `RUSTYGATE_CONFIG`.
2. Verify `/health`, `/ready`, and `./scripts/smoke.sh`.
3. Send SIGTERM to the old instance.
4. Confirm logs show the shutdown signal and no repeated startup validation failures.

If the grace period elapses, the process exits with an error so the supervisor can restart or alert.

## Rollback

Rollback should be config-first when possible:

1. Restore the previous `RUSTYGATE_CONFIG` file or image tag.
2. Keep the same SQLite volume mounted if request history should remain available.
3. Restart the service and run the smoke check.
4. Watch `/metrics` for error-rate, timeout, and provider error counters.

SQLite schema migrations are intentionally simple. Before deploying a build with persistence changes, keep a copy of the SQLite file or volume snapshot.

## Observability Access

`/stats`, `/stats/providers`, and `/metrics` require an API key with an observability-capable role. In SQLite-backed staging/prod deployments, create a separate `observability` key instead of reusing an inference client key. Keep that key scoped to operators and monitoring systems, and keep these routes behind the same internal network boundary as the gateway listener.

Do not expose metrics or stats endpoints directly to the public internet. If a reverse proxy or service mesh fronts RustyGate, route observability paths only from monitoring networks or admin VPNs.

## Request IDs

RustyGate attaches a UUID request ID to protected-route errors and request logs. Clients may provide `X-Request-Id` with a UUID value; otherwise RustyGate generates one. The same request ID is used across global rate-limit, auth, per-key rate-limit, route validation, and admission errors for a single inbound request.

Use the request ID from an error response when correlating client failures with logs:

```json
{
  "error": {
    "code": "admission_rejected",
    "message": "gateway capacity is saturated, retry later",
    "request_id": "00000000-0000-0000-0000-000000000000"
  }
}
```

## Key Rotation

Use overlapping keys so clients can move without downtime:

1. Create a new key with the same intended role and limits, or narrower limits if the rotation is also a scope reduction.
2. Deploy clients with the new key and confirm successful `/v1/models` or inference smoke checks.
3. Verify quota and role scope for the new key by checking expected success and forbidden paths, for example inference keys should not read `/stats`.
4. Revoke the old key after traffic has drained from clients using it.
5. Confirm the old key receives `401` and watch `/metrics` or request logs for unexpected auth failures.

Prefer separate keys for inference clients, observability systems, and admin operations. Keep old keys active only for the planned overlap window.

## Incident Triage

Start with:

- `/health` and `/ready` for process liveness.
- `/metrics` for request failures, admission rejections, in-flight requests, provider timeout/rate-limit counters, and p95 latency.
- `/stats/providers` for provider-specific error categories and fallback attempts.
- Startup logs for config validation errors, loaded provider names, storage mode, and prompt logging status.

Common causes:

- `401` responses: missing or wrong `Authorization: Bearer` value.
- `400` with estimated-token messages: the request exceeds `gateway.admission.max_estimated_prompt_tokens` or `gateway.admission.max_estimated_total_tokens`.
- `503` with `admission_rejected`: global, model-pool, or provider in-flight admission limits are saturated; clients should honor `Retry-After`.
- Streaming SSE error after partial output: the upstream provider failed, ended without a completion event, or exceeded `gateway.stream_idle_timeout_ms` between chunks; check `rustygate_stream_outcomes_total` and provider error categories.
- Startup failure: missing gateway/provider env vars, duplicate providers, invalid limits, or an alias targeting a model no provider serves.
- Provider failures: upstream timeout, rate limiting, or an open circuit breaker.
- SQLite errors: missing `/data` volume, unwritable database path, or a stale file permission issue.

## Admission Tuning

Admission controls are immediate rejection gates, not background queues. Set them conservatively for the deployment profile:

- Use `gateway.admission.max_global_in_flight` to cap total inference work accepted by this RustyGate process.
- Use model-pool `max_in_flight` for shared self-hosted replica pools.
- Use provider `max_in_flight` for a single upstream or replica that should not receive more than a fixed number of concurrent attempts.
- Use estimated-token limits to reject prompts or generations that are too large for the intended demo/runtime profile.

The counters are process-local. If multiple RustyGate instances run behind a load balancer, each instance enforces its own admission limits.

## Streaming Tuning

`gateway.stream_idle_timeout_ms` bounds the maximum idle gap between chunks once a provider stream has started. Set it higher than normal provider token gaps, but below the point where a stuck stream would tie up admission and provider in-flight capacity for too long.

Dropped downstream SSE bodies release RustyGate's in-flight and admission guards when Axum drops the response body. This is best-effort cancellation cleanup; RustyGate stops polling the upstream stream rather than buffering output after the client disconnects.

Do not enable `gateway.log_prompt_content` outside local debugging.

Compatibility resource endpoints return OpenAI-shaped placeholders for SDK compatibility when `gateway.route_exposure.placeholder_compat_routes = true`. Realtime `client_secret` values generated by RustyGate are synthetic IDs, not provider-issued credentials.
