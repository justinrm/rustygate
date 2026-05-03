# Caching

RustyGate supports opt-in exact-match response caching for non-streaming, deterministic requests.

## Exact-Match Cache

Enable it in TOML:

```toml
[cache]
enabled = true
backend = "memory" # or "sqlite"
default_ttl_seconds = 600
max_entries = 10000
```

The cache key is a SHA-256 hash over the canonicalized model, messages, tools, tool choice, response format, `max_tokens`, and `parallel_tool_calls`. Requests are skipped when `stream = true` or `temperature > 0`.

Responses include `X-RustyGate-Cache: HIT` or `MISS` when caching is enabled. Cache activity is exposed as:

- `rustygate_cache_lookups_total{outcome="hit|miss|skip_streaming|skip_temperature|skip_tools|skip_disabled"}`
- `rustygate_cache_hit_ratio`

## Experimental Semantic Cache

Semantic caching is behind the `semantic-cache` Cargo feature:

```sh
cargo test --features semantic-cache
```

Semantic caching wraps the exact-match cache with an embedding similarity layer. Configure it explicitly:

```toml
[cache.semantic]
enabled = true
embedding_provider = "openai-primary"
similarity_threshold = 0.95
index_capacity = 10000
```

It stays disabled by default because semantic caching has privacy, cost, and correctness tradeoffs: prompts must be embedded, similarity thresholds need tuning, and approximate hits can return stale or semantically adjacent answers.
