-- Task management tables for /api/tasks backend.
-- This schema is aligned with docs/task-management-panel.md and the
-- TypeScript domain model in web/src/domain/tasks.ts.

-- Core task metadata. Each row represents a single logical task that may
-- touch one or more units.
CREATE TABLE IF NOT EXISTS tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Public task identifier used in URLs and API paths (/api/tasks/:id).
    task_id TEXT NOT NULL UNIQUE,

    -- High-level task kind, e.g. manual / github-webhook / scheduler / maintenance / internal / other.
    kind TEXT NOT NULL,

    -- Current status: pending / running / succeeded / failed / cancelled / skipped.
    status TEXT NOT NULL,

    -- Timestamps in Unix seconds.
    created_at INTEGER NOT NULL,
    started_at INTEGER,
    finished_at INTEGER,
    updated_at INTEGER,

    -- Short summary for list display.
    summary TEXT,

    -- Opaque JSON payload, used by the task executor to carry
    -- kind-specific parameters (e.g. manual dry_run flags, github image).
    meta TEXT,

    -- Trigger metadata, flattened for ease of querying.
    trigger_source TEXT NOT NULL,
    trigger_request_id TEXT,
    trigger_path TEXT,
    trigger_caller TEXT,
    trigger_reason TEXT,
    trigger_scheduler_iteration INTEGER,

    -- Control hints for the UI.
    can_stop INTEGER NOT NULL DEFAULT 0,
    can_force_stop INTEGER NOT NULL DEFAULT 0,
    can_retry INTEGER NOT NULL DEFAULT 0,

    -- Whether the task is expected to be long-running.
    is_long_running INTEGER,

    -- If this task is a retry of another task, point to the original task_id.
    retry_of TEXT
);

CREATE INDEX IF NOT EXISTS idx_tasks_created_at ON tasks (created_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks (status);
CREATE INDEX IF NOT EXISTS idx_tasks_kind ON tasks (kind);
CREATE INDEX IF NOT EXISTS idx_tasks_retry_of ON tasks (retry_of);

-- Unit-level status for each task. A task may involve one or more units.
CREATE TABLE IF NOT EXISTS task_units (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    unit TEXT NOT NULL,
    slug TEXT,
    display_name TEXT,
    status TEXT NOT NULL,
    phase TEXT,
    started_at INTEGER,
    finished_at INTEGER,
    duration_ms INTEGER,
    message TEXT,
    error TEXT,
    FOREIGN KEY (task_id) REFERENCES tasks (task_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_task_units_task_id ON task_units (task_id);
CREATE INDEX IF NOT EXISTS idx_task_units_unit ON task_units (unit);

-- Task-local log timeline. This is used by /api/tasks/:id to render a
-- per-task execution trace in the UI.
CREATE TABLE IF NOT EXISTS task_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    ts INTEGER NOT NULL,
    level TEXT NOT NULL,
    action TEXT NOT NULL,
    status TEXT NOT NULL,
    summary TEXT NOT NULL,
    unit TEXT,
    meta TEXT,
    FOREIGN KEY (task_id) REFERENCES tasks (task_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_task_logs_task_ts ON task_logs (task_id, ts, id);

-- Optional linkage from generic event_log rows back to tasks for
-- cross-navigation can be added in a future, more controlled migration.
