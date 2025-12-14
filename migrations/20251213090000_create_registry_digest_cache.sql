-- Cache for remote registry manifest digests keyed by normalized image ref.
-- Used by /manual update indicators (digest-only comparison).

CREATE TABLE IF NOT EXISTS registry_digest_cache (
    -- Normalized image reference: registry/repo:tag (no scheme).
    image TEXT PRIMARY KEY,
    -- Remote manifest digest (e.g. sha256:...), nullable on errors.
    digest TEXT,
    -- Unix seconds when we last attempted to check the remote digest.
    checked_at INTEGER NOT NULL,
    -- ok | error
    status TEXT NOT NULL,
    -- Short, sanitized error code (no credentials).
    error TEXT
);

CREATE INDEX IF NOT EXISTS idx_registry_digest_cache_checked_at
ON registry_digest_cache (checked_at);
