CREATE TABLE IF NOT EXISTS rate_limit_tokens (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    scope TEXT NOT NULL,
    bucket TEXT NOT NULL,
    ts INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_rate_limit_scope_bucket_ts
    ON rate_limit_tokens (scope, bucket, ts);

CREATE TABLE IF NOT EXISTS image_locks (
    bucket TEXT PRIMARY KEY,
    acquired_at INTEGER NOT NULL
);
