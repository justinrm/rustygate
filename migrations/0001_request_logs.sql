CREATE TABLE request_logs (
    id TEXT PRIMARY KEY,
    route TEXT NOT NULL,
    created_at_unix_seconds INTEGER NOT NULL,
    requested_model TEXT,
    final_provider TEXT,
    status TEXT NOT NULL,
    latency_ms INTEGER NOT NULL,
    prompt_tokens INTEGER,
    completion_tokens INTEGER,
    total_tokens INTEGER,
    input_cost_usd REAL,
    output_cost_usd REAL,
    total_cost_usd REAL,
    error_category TEXT,
    prompt_messages_json TEXT
);

CREATE TABLE provider_attempts (
    id TEXT PRIMARY KEY,
    request_id TEXT NOT NULL,
    provider_name TEXT NOT NULL,
    attempt_order INTEGER NOT NULL,
    success INTEGER NOT NULL,
    is_fallback INTEGER NOT NULL,
    error_category TEXT,
    latency_ms INTEGER NOT NULL,
    FOREIGN KEY(request_id) REFERENCES request_logs(id)
);
