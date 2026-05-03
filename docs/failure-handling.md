# Failure Handling

RustyGate should classify failures clearly and return clean JSON errors. Client-facing errors must not expose secrets, raw provider payloads, stack traces, or Authorization headers.

## Error Categories

- `InvalidRequest`: request shape or validation failure
- `Timeout`: provider did not respond within the configured timeout
- `RateLimited`: provider returned a rate limit response
- `AuthenticationFailed`: provider credentials failed
- `ProviderUnavailable`: provider is temporarily unavailable
- `ProviderBadResponse`: provider returned malformed or unexpected data
- `NoProviderAvailable`: routing could not find a usable provider
- `Internal`: unexpected gateway failure

## Status Code Mappings

- `400`: invalid request
- `401`: authentication failure if gateway auth is added later
- `429`: rate limit
- `502`: upstream provider error
- `503`: no provider available
- `504`: timeout
- `500`: unknown internal failure

## Retry Policy

Retry only when the failure is likely temporary. The current MVP does not retry the same provider; it falls back to the next matching provider once per provider. The parsed `gateway.max_retries` setting is reserved for future same-provider retry support.

Retry candidates:

- Timeout
- Rate limited, when another matching provider is available
- Provider unavailable

## Fallback Policy

Fallback should try the next matching provider in priority order after a retryable failure. Fallback should record which providers were attempted and why each attempt failed.

## What Not To Retry

Do not retry:

- Invalid request payloads
- Authentication failures
- Missing provider configuration
- Unsupported models
- Responses that cannot be safely normalized

## Secret Redaction Rules

- Never log API keys.
- Never log Authorization headers.
- Never log prompt content by default.
- Never return full provider raw errors to clients.
- Prefer request IDs and classified errors for debugging.
