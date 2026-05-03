CREATE TABLE api_keys (
    id TEXT PRIMARY KEY,
    key_prefix TEXT NOT NULL,
    key_hash TEXT NOT NULL,
    label TEXT NOT NULL,
    role TEXT NOT NULL,
    requests_per_minute INTEGER,
    daily_token_quota INTEGER,
    daily_cost_quota_usd REAL,
    cache_enabled INTEGER NOT NULL DEFAULT 1,
    created_at_unix_seconds INTEGER NOT NULL,
    revoked_at_unix_seconds INTEGER
);

CREATE UNIQUE INDEX api_keys_prefix ON api_keys(key_prefix);

CREATE TABLE api_key_usage_daily (
    api_key_id TEXT NOT NULL,
    day_unix INTEGER NOT NULL,
    request_count INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    total_cost_usd REAL NOT NULL DEFAULT 0,
    PRIMARY KEY (api_key_id, day_unix),
    FOREIGN KEY(api_key_id) REFERENCES api_keys(id)
);
