import { z } from 'zod'

// Shared helpers

const fileStatsSchema = z
  .object({
    exists: z.boolean(),
    is_dir: z.boolean().optional(),
    size: z.number().optional(),
    modified_ts: z.number().nullable().optional(),
    path: z.string().optional(),
  })
  .passthrough()

// /api/settings

export const settingsSchema = z
  .object({
    env: z
      .object({
        PODUP_STATE_DIR: z.string().optional(),
        PODUP_TOKEN_configured: z.boolean().optional(),
        PODUP_MANUAL_TOKEN_configured: z.boolean().optional(),
        PODUP_GH_WEBHOOK_SECRET_configured: z.boolean().optional(),
      })
      .passthrough(),
    scheduler: z
      .object({
        interval_secs: z.number().optional(),
        min_interval_secs: z.number().optional(),
        max_iterations: z.number().nullable().optional(),
        recent_events: z
          .array(
            z
              .object({
                iteration: z.number().nullable().optional(),
              })
              .passthrough(),
          )
          .optional(),
      })
      .passthrough(),
    systemd: z
      .object({
        auto_update_unit: z.string().optional(),
        trigger_units: z.array(z.string()).optional(),
      })
      .passthrough(),
    database: z
      .object({
        url: z.string().optional(),
      })
      .passthrough(),
    version: z
      .object({
        package: z.string().optional(),
        build_timestamp: z.string().nullable().optional(),
      })
      .passthrough(),
    forward_auth: z
      .object({
        header: z.string().nullable().optional(),
        admin_value_configured: z.boolean().optional(),
        nickname_header: z.string().nullable().optional(),
        admin_mode_name: z.string().nullable().optional(),
        dev_open_admin: z.boolean().optional(),
        mode: z.string().optional(),
      })
      .passthrough(),
    resources: z
      .object({
        state_dir: z
          .object({
            path: z.string().optional(),
          })
          .optional(),
        database_file: fileStatsSchema.optional(),
        debug_payload: fileStatsSchema.optional(),
        web_dist: fileStatsSchema.optional(),
      })
      .partial()
      .optional(),
  })
  .passthrough()

// /api/webhooks/status

const webhookUnitSchema = z
  .object({
    unit: z.string(),
    slug: z.string(),
    webhook_url: z.string(),
    redeploy_url: z.string(),
    expected_image: z.string().nullable().optional(),
    last_ts: z.number().nullable().optional(),
    last_status: z.number().nullable().optional(),
    last_request_id: z.string().nullable().optional(),
    last_success_ts: z.number().nullable().optional(),
    last_failure_ts: z.number().nullable().optional(),
    hmac_ok: z.boolean(),
    hmac_last_error: z.string().nullable().optional(),
  })
  .passthrough()

export const webhooksStatusSchema = z
  .object({
    now: z.number(),
    secret_configured: z.boolean(),
    units: z.array(webhookUnitSchema),
  })
  .passthrough()

// /api/tasks list + detail

const taskStatusSchema = z.enum([
  'pending',
  'running',
  'succeeded',
  'failed',
  'cancelled',
  'skipped',
])

const taskKindSchema = z.enum([
  'manual',
  'github-webhook',
  'scheduler',
  'maintenance',
  'internal',
  'other',
])

const taskTriggerSchema = z
  .object({
    source: z.enum(['manual', 'webhook', 'scheduler', 'maintenance', 'cli', 'system']),
    request_id: z.string().nullable().optional(),
    path: z.string().nullable().optional(),
    caller: z.string().nullable().optional(),
    reason: z.string().nullable().optional(),
    scheduler_iteration: z.number().nullable().optional(),
  })
  .passthrough()

const taskUnitSummarySchema = z
  .object({
    unit: z.string(),
    slug: z.string().optional(),
    display_name: z.string().optional(),
    status: taskStatusSchema,
    phase: z
      .enum(['queued', 'pulling-image', 'restarting', 'waiting', 'verifying', 'done'])
      .optional(),
    started_at: z.number().nullable().optional(),
    finished_at: z.number().nullable().optional(),
    duration_ms: z.number().nullable().optional(),
    message: z.string().nullable().optional(),
    error: z.string().nullable().optional(),
  })
  .passthrough()

const taskSummaryCountsSchema = z
  .object({
    total_units: z.number(),
    succeeded: z.number(),
    failed: z.number(),
    cancelled: z.number(),
    running: z.number(),
    pending: z.number(),
    skipped: z.number(),
  })
  .passthrough()

const taskSchema = z
  .object({
    id: z.number(),
    task_id: z.string(),
    kind: taskKindSchema,
    status: taskStatusSchema,
    created_at: z.number(),
    started_at: z.number().nullable().optional(),
    finished_at: z.number().nullable().optional(),
    updated_at: z.number().nullable().optional(),
    summary: z.string().nullable().optional(),
    trigger: taskTriggerSchema,
    units: z.array(taskUnitSummarySchema),
    unit_counts: taskSummaryCountsSchema,
    can_stop: z.boolean(),
    can_force_stop: z.boolean(),
    can_retry: z.boolean(),
    is_long_running: z.boolean().optional(),
    retry_of: z.string().nullable().optional(),
  })
  .passthrough()

const taskLogEntrySchema = z
  .object({
    id: z.number(),
    ts: z.number(),
    level: z.enum(['info', 'warning', 'error']),
    action: z.string(),
    status: taskStatusSchema,
    summary: z.string(),
    unit: z.string().nullable().optional(),
    meta: z.unknown().optional(),
  })
  .passthrough()

export const tasksListResponseSchema = z
  .object({
    tasks: z.array(taskSchema),
    total: z.number(),
    page: z.number(),
    page_size: z.number(),
    has_next: z.boolean(),
  })
  .passthrough()

export const taskDetailResponseSchema = taskSchema
  .extend({
    logs: z.array(taskLogEntrySchema),
  })
  .passthrough()

// Utility used from handlers to perform optional schema validation.
export function validateMockResponse(
  schema: z.ZodTypeAny,
  value: unknown,
  context: { path: string },
) {
  const result = schema.safeParse(value)
  if (!result.success) {
    const issues = result.error.issues.map((issue) => {
      const path = issue.path.join('.') || '(root)'
      return `${path}: ${issue.message}`
    })
    // Non-fatal contract warning for mock responses only.
    console.warn(
      '[mock-zod]',
      `Response for ${context.path} did not match mock schema`,
      issues.join('; '),
    )
  }
}
