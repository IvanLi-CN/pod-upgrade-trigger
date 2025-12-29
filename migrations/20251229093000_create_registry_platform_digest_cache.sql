-- Cache for remote registry digests keyed by normalized image ref + host platform.
-- Used by task image-verify (remote_index_digest + remote_platform_digest).

CREATE TABLE IF NOT EXISTS registry_platform_digest_cache (
    -- Normalized image reference: registry/repo:tag (no scheme).
    image TEXT NOT NULL,
    -- OCI platform triple used to select the correct manifest from an index/list.
    platform_os TEXT NOT NULL,
    platform_arch TEXT NOT NULL,
    -- Variant is stored as a non-null string to keep the composite primary key stable.
    -- Empty string means "no variant".
    platform_variant TEXT NOT NULL DEFAULT '',
    -- Remote manifest list/index digest (or single-manifest digest).
    remote_index_digest TEXT,
    -- Remote platform manifest digest selected from the index/list.
    remote_platform_digest TEXT,
    -- Unix seconds when we last attempted to check the remote digest.
    checked_at INTEGER NOT NULL,
    -- ok | error
    status TEXT NOT NULL,
    -- Short, sanitized error code (no credentials).
    error TEXT,
    PRIMARY KEY (image, platform_os, platform_arch, platform_variant)
);

CREATE INDEX IF NOT EXISTS idx_registry_platform_digest_cache_checked_at
ON registry_platform_digest_cache (checked_at);

