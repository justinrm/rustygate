# OpenAI Compatibility

RustyGate targets practical OpenAI API compatibility for portfolio and local gateway demos. The implementation is Responses-first and keeps gateway operations (`/stats`, `/metrics`, provider routing, fallback, request logs) separate from OpenAI-shaped API responses.

## Compatibility Matrix

| Endpoint | Status | Notes |
| --- | --- | --- |
| `POST /v1/responses` | Provider-backed | Uses existing provider routing/fallback. Supports JSON and SSE response events. |
| `POST /v1/chat/completions` | Provider-backed | Legacy surface remains available with OpenAI-style completion IDs and streaming chunks. |
| `GET /v1/models` | Gateway-backed | Returns OpenAI-shaped model list objects from configured providers, aliases, and model pools. |
| `POST /v1/embeddings` | Placeholder shape | Returns deterministic lightweight embeddings for compatibility tests and SDK smoke checks. |
| `POST /v1/moderations` | Placeholder shape | Returns non-flagged moderation results. |
| `POST /v1/images/generations` | Placeholder shape | Returns placeholder image URLs. |
| `POST /v1/images/edits` | Placeholder shape | Accepts request bodies and returns placeholder image URLs. |
| `POST /v1/images/variations` | Placeholder shape | Accepts request bodies and returns placeholder image URLs. |
| `POST /v1/audio/transcriptions` | Placeholder shape | Accepts request bodies and returns a transcription text field. |
| `POST /v1/audio/translations` | Placeholder shape | Accepts request bodies and returns a translation text field. |
| `/v1/files` | Placeholder shape | Lists, creates, retrieves, deletes, and returns content for lightweight file objects. |
| `/v1/batches` | Placeholder shape | Creates, retrieves, cancels, and lists lightweight batch objects. |
| `/v1/fine_tuning/jobs` | Placeholder shape | Creates, retrieves, cancels, lists jobs, and lists events. |
| `POST /v1/realtime/sessions` | Placeholder shape | Creates ephemeral realtime session metadata and client secrets. |

## Route Exposure

Local and SDK-compatibility demos can leave placeholder routes enabled. Internal staging or production-style deployments can expose only real inference/model endpoints with:

```toml
[gateway.route_exposure]
placeholder_compat_routes = false
```

When disabled, placeholder endpoint families return `404`. `/v1/responses`, `/v1/chat/completions`, `/v1/models`, `/health`, and `/ready` remain available.

## Acceptance Checks

- Endpoint integration tests cover Responses, streaming Responses, embeddings, resources, and realtime session creation.
- `scripts/smoke.sh` checks `/v1/responses` alongside health, chat, and stats.
- `scripts/openai_sdk_smoke.py` optionally validates the official OpenAI Python SDK against `/v1/models`, `/v1/responses`, and `/v1/embeddings`.

## Deliberate Tradeoffs

Some endpoint families currently provide OpenAI-shaped lightweight behavior rather than complete provider-backed execution. This keeps the project small while creating stable contracts and test coverage that can be deepened endpoint by endpoint. Disable those placeholders for production-style internal deployments where exposing only real provider-backed paths is clearer.
