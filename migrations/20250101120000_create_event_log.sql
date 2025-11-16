CREATE TABLE IF NOT EXISTS event_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    request_id TEXT NOT NULL,
    ts INTEGER NOT NULL,
    method TEXT NOT NULL,
    path TEXT,
    status INTEGER NOT NULL,
    action TEXT NOT NULL,
    duration_ms INTEGER NOT NULL,
    meta TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);

CREATE INDEX IF NOT EXISTS idx_event_log_ts ON event_log (ts);
CREATE INDEX IF NOT EXISTS idx_event_log_action ON event_log (action);
