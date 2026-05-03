# OpenAI Compatibility

RustyGate targets practical OpenAI API compatibility for portfolio and local gateway demos. The implementation is Responses-first and keeps gateway operations (`/stats`, `/metrics`, provider routing, fallback, request logs) separate from OpenAI-shaped API responses.

## Compatibility Matrix

| Endpoint | Status | Notes |
| --- | --- | --- |
| `POST /v1/responses` | Supported | Uses existing provider routing/fallback. Supports JSON and SSE response events. |
| `POST /v1/chat/completions` | Supported | Legacy surface remains available with OpenAI-style completion IDs and streaming chunks. |
| `GET /v1/models` | Supported | Returns OpenAI-shaped model list objects. |
| `POST /v1/embeddings` | Supported shape | Returns deterministic lightweight embeddings for compatibility tests and SDK smoke checks. |
| `POST /v1/moderations` | Supported shape | Returns non-flagged moderation results. |
| `POST /v1/images/generations` | Supported shape | Returns placeholder image URLs. |
| `POST /v1/images/edits` | Supported shape | Accepts request bodies and returns placeholder image URLs. |
| `POST /v1/images/variations` | Supported shape | Accepts request bodies and returns placeholder image URLs. |
| `POST /v1/audio/transcriptions` | Supported shape | Accepts request bodies and returns a transcription text field. |
| `POST /v1/audio/translations` | Supported shape | Accepts request bodies and returns a translation text field. |
| `/v1/files` | Supported shape | Lists, creates, retrieves, deletes, and returns content for lightweight file objects. |
| `/v1/batches` | Supported shape | Creates, retrieves, cancels, and lists lightweight batch objects. |
| `/v1/fine_tuning/jobs` | Supported shape | Creates, retrieves, cancels, lists jobs, and lists events. |
| `POST /v1/realtime/sessions` | Supported shape | Creates ephemeral realtime session metadata and client secrets. |

## Acceptance Checks

- Endpoint integration tests cover Responses, streaming Responses, embeddings, resources, and realtime session creation.
- `scripts/smoke.sh` checks `/v1/responses` alongside health, chat, and stats.
- `scripts/openai_sdk_smoke.py` optionally validates the official OpenAI Python SDK against `/v1/models`, `/v1/responses`, and `/v1/embeddings`.

## Deliberate Tradeoffs

Some endpoint families currently provide OpenAI-shaped lightweight behavior rather than complete provider-backed execution. This keeps the project small while creating stable contracts and test coverage that can be deepened endpoint by endpoint.
