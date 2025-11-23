CREATE TABLE IF NOT EXISTS discovered_units (
    unit TEXT PRIMARY KEY,
    source TEXT NOT NULL DEFAULT 'podman',
    discovered_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_discovered_units_source ON discovered_units (source);
