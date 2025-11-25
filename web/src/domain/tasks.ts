export type TaskStatus =
  | 'pending'
  | 'running'
  | 'succeeded'
  | 'failed'
  | 'cancelled'
  | 'skipped'

export type TaskKind =
  | 'manual'
  | 'github-webhook'
  | 'scheduler'
  | 'maintenance'
  | 'internal'
  | 'other'

export type TaskTriggerSource = 'manual' | 'webhook' | 'scheduler' | 'maintenance' | 'cli' | 'system'

export type TaskTriggerMeta = {
  /**
   * High-level source of the task, e.g. manual button, webhook, scheduler tick.
   */
  source: TaskTriggerSource
  /**
   * Correlated HTTP/system request id, if available.
   */
  request_id?: string | null
  /**
   * Originating HTTP path or CLI command, if applicable.
   */
  path?: string | null
  /**
   * Manual caller identity (from UI/API), when present.
   */
  caller?: string | null
  /**
   * Free-form reason text for manual/scheduled tasks.
   */
  reason?: string | null
  /**
   * Scheduler iteration index if the task was started by a scheduler loop.
   */
  scheduler_iteration?: number | null
}

export type TaskUnitStatus = TaskStatus

export type TaskUnitSummary = {
  /**
   * Systemd unit name, e.g. svc-alpha.service.
   */
  unit: string
  /**
   * Short slug used elsewhere in the UI; optional for non-service tasks.
   */
  slug?: string
  /**
   * Optional human-readable display name.
   */
  display_name?: string
  status: TaskUnitStatus
  /**
   * Optional phase hint for the unit-level work.
   * This is purely for UX and does not affect state transitions.
   */
  phase?:
    | 'queued'
    | 'pulling-image'
    | 'restarting'
    | 'waiting'
    | 'verifying'
    | 'done'
  started_at?: number | null
  finished_at?: number | null
  duration_ms?: number | null
  /**
   * Short, user-facing message summarizing the outcome for this unit.
   */
  message?: string | null
  /**
   * Optional error string when the unit failed or was aborted.
   */
  error?: string | null
}

export type TaskSummaryCounts = {
  total_units: number
  succeeded: number
  failed: number
  cancelled: number
  running: number
  pending: number
  skipped: number
}

export type Task = {
  /**
   * Internal numeric id; mainly useful for stable sorting.
   */
  id: number
  /**
   * Public task identifier used for routing and correlation.
   */
  task_id: string
  kind: TaskKind
  status: TaskStatus
  created_at: number
  started_at?: number | null
  finished_at?: number | null
  updated_at?: number | null
  /**
   * Short summary string for list display, e.g. "3/5 units succeeded".
   */
  summary?: string | null
  trigger: TaskTriggerMeta
  units: TaskUnitSummary[]
  unit_counts: TaskSummaryCounts
  /**
   * Control hints used by the UI to decide which actions to show.
   */
  can_stop: boolean
  can_force_stop: boolean
  can_retry: boolean
  /**
   * Whether this task is expected to take noticeable time and thus
   * should default to drawer auto-open behaviour.
   */
  is_long_running?: boolean
  /**
   * When present, points to the original task that this one retries.
   */
  retry_of?: string | null
}

export type TaskLogLevel = 'info' | 'warning' | 'error'

export type TaskLogEntry = {
  id: number
  ts: number
  level: TaskLogLevel
  /**
   * High-level action name for the timeline, e.g. "image-pull", "restart-unit".
   */
  action: string
  /**
   * Status of this step, mapped to the same vocabulary as TaskStatus.
   */
  status: TaskStatus
  /**
   * Human-friendly summary for timeline display.
   */
  summary: string
  /**
   * Optional unit this log line relates to.
   */
  unit?: string | null
  /**
   * Raw metadata attached to the event, suitable for JSON inspector views.
   */
  meta?: unknown
}

export type TasksListResponse = {
  tasks: Task[]
  total: number
  page: number
  page_size: number
  has_next: boolean
}

export type TaskDetailResponse = Task & {
  logs: TaskLogEntry[]
}

