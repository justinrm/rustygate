CREATE TABLE cache_entries (
    cache_key TEXT PRIMARY KEY,
    response_json TEXT NOT NULL,
    expires_at_unix_seconds INTEGER NOT NULL
);

CREATE INDEX cache_entries_expires_at ON cache_entries(expires_at_unix_seconds);
