# Roadmap

RustyGate should grow in small, reviewable steps.

## v0.1.0 Baseline Limitations

- No streaming chat completions
- No gateway authentication on chat and stats endpoints
- No gateway rate limiting
- No provider circuit breaker

## MVP

- Axum service with `/health` and `/ready`
- Simplified chat completion request and response models
- Mock provider with deterministic responses
- Basic request validation
- Priority routing by exact model match
- Retry and fallback for retryable provider failures
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

## MVP+

- Better token estimation
- Optional default model configuration

## Stretch Goals

- More advanced persistence and retention controls
- Provider health checks
- Circuit breaker
- Cost-aware routing
- Model aliases
- Prometheus metrics
- OpenTelemetry traces

## Do Not Build Yet

- Streaming responses
- Kubernetes manifests
- Multi-user billing
- Complex authentication
- Redis
- Web dashboard
- Semantic caching
- Production-grade policy engine
- Full OpenAI API compatibility
