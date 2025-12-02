ALTER TABLE event_log
ADD COLUMN task_id TEXT;

CREATE INDEX IF NOT EXISTS idx_event_log_task_id ON event_log (task_id);

