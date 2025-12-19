/// <reference lib="dom" />
import { faker } from '@faker-js/faker'
import cloneDeep from 'lodash-es/cloneDeep'
import type {
  Task,
  TaskLogEntry,
  TaskSummaryCounts,
  TaskTriggerMeta,
  TaskUnitSummary,
} from '../domain/tasks'

export type MockProfile =
  | 'happy-path'
  | 'empty-state'
  | 'rate-limit-hot'
  | 'auth-error'
  | 'auth-error'
  | 'degraded'

export type MockEvent = {
  id: number
  request_id: string
  ts: number
  method: string
  path: string | null
  status: number
  action: string
  duration_ms: number
  meta: unknown
  task_id?: string | null
  created_at: number
}

export type ManualServiceUpdate = {
  status: 'tag_update_available' | 'latest_ahead' | 'up_to_date' | 'unknown'
  tag?: string
  running_digest?: string
  remote_tag_digest?: string
  remote_latest_digest?: string
  checked_at?: number
  stale?: boolean
  reason?: string
}

export type ManualService = {
  slug: string
  unit: string
  display_name: string
  default_image?: string | null
  github_path?: string
  is_auto_update?: boolean
  update?: ManualServiceUpdate | null
}

export type WebhookUnit = {
  unit: string
  slug: string
  webhook_url: string
  redeploy_url: string
  expected_image?: string | null
  last_ts?: number | null
  last_status?: number | null
  last_request_id?: string | null
  last_success_ts?: number | null
  last_failure_ts?: number | null
  hmac_ok: boolean
  hmac_last_error?: string | null
}

export type WebhooksStatus = {
  now: number
  secret_configured: boolean
  units: WebhookUnit[]
}

export type LockEntry = {
  bucket: string
  acquired_at: number
  age_secs: number
}

export type SettingsSnapshot = {
  env: {
    PODUP_STATE_DIR?: string
    PODUP_TOKEN_configured?: boolean
    PODUP_GH_WEBHOOK_SECRET_configured?: boolean
  }
  scheduler: {
    interval_secs?: number
    min_interval_secs?: number
    max_iterations?: number | null
    recent_events?: { iteration?: number | null }[]
  }
  systemd: {
    auto_update_unit?: string
    trigger_units?: string[]
  }
  database: {
    url?: string
  }
  version: {
    package?: string
    release_tag?: string | null
    build_timestamp?: string | null
  }
  forward_auth: {
    header?: string | null
    admin_value_configured?: boolean
    nickname_header?: string | null
    admin_mode_name?: string | null
    dev_open_admin?: boolean
    mode?: string
  }
  resources?: {
    state_dir?: { path?: string }
    database_file?: FileStats
    debug_payload?: FileStats
    web_dist?: FileStats
  }
}

export type FileStats = {
  exists: boolean
  is_dir?: boolean
  size?: number
  modified_ts?: number | null
  path?: string
}

export type ConfigSnapshot = {
  web?: {
    webhook_url_prefix?: string | null
    github_webhook_path_prefix?: string | null
  }
}

export type RuntimeData = {
  now: number
  events: MockEvent[]
  services: ManualService[]
  webhooks: WebhooksStatus
  locks: LockEntry[]
  settings: SettingsSnapshot
  config: ConfigSnapshot
  lastPayload: Uint8Array
  tasks: Task[]
  taskLogs: Record<string, TaskLogEntry[]>
}

export type RuntimeSnapshot = {
  profile: MockProfile
  delayMs: number
  errorRate: number
  data: RuntimeData
}

type Listener = (snapshot: RuntimeSnapshot) => void

const STORAGE_KEYS = {
  profile: 'mock:profile',
  delay: 'mock:delay',
  error: 'mock:error-rate',
} as const

function makeRequestId() {
  return faker.string.alphanumeric(12).toLowerCase()
}

function summarizeUnits(units: TaskUnitSummary[]): TaskSummaryCounts {
  const summary: TaskSummaryCounts = {
    total_units: units.length,
    succeeded: 0,
    failed: 0,
    cancelled: 0,
    running: 0,
    pending: 0,
    skipped: 0,
  }
  for (const unit of units) {
    switch (unit.status) {
      case 'succeeded':
        summary.succeeded += 1
        break
      case 'failed':
        summary.failed += 1
        break
      case 'cancelled':
        summary.cancelled += 1
        break
      case 'running':
        summary.running += 1
        break
      case 'pending':
        summary.pending += 1
        break
      case 'skipped':
        summary.skipped += 1
        break
      default:
        break
    }
  }
  return summary
}

function buildEvents(now: number, profile: MockProfile, tasks: Task[]): MockEvent[] {
  if (profile === 'empty-state') return []

  const baseTs = now - 3600
  const nightlyTask = tasks.find((task) =>
    (task.summary ?? '').includes('nightly manual upgrade'),
  )
  const nightlyTaskId = nightlyTask?.task_id ?? null

  const common: MockEvent[] = [
    {
      id: 1,
      request_id: makeRequestId(),
      ts: baseTs + 120,
      method: 'POST',
      path: '/api/manual/deploy',
      status: 200,
      action: 'manual-trigger',
      duration_ms: 320,
      meta: {
        caller: 'seed',
        reason: 'daily maintenance',
        ...(nightlyTaskId ? { task_id: nightlyTaskId } : {}),
      },
      task_id: nightlyTaskId,
      created_at: baseTs + 120,
    },
    {
      id: 2,
      request_id: makeRequestId(),
      ts: baseTs + 900,
      method: 'POST',
      path: '/github-package-update/svc-alpha',
      status: 200,
      action: 'github-webhook',
      duration_ms: 210,
      meta: { package: 'svc-alpha', tags: ['latest'] },
      created_at: baseTs + 900,
    },
    {
      id: 3,
      request_id: makeRequestId(),
      ts: baseTs + 1100,
      method: 'GET',
      path: '/api/events',
      status: 200,
      action: 'scheduler',
      duration_ms: 120,
      meta: { iteration: 42 },
      created_at: baseTs + 1100,
    },
  ]

  if (profile === 'rate-limit-hot') {
    const heavy: MockEvent[] = Array.from({ length: 12 }).map((_, idx) => ({
      id: 100 + idx,
      request_id: makeRequestId(),
      ts: baseTs + 60 * idx,
      method: 'POST',
      path: '/api/manual/services/svc-beta',
      status: idx % 4 === 0 ? 429 : 200,
      action: 'manual-trigger',
      duration_ms: 180 + idx,
      meta: { burst: true, idx },
      created_at: baseTs + 60 * idx,
    }))
    return [...heavy, ...common].sort((a, b) => b.ts - a.ts)
  }

  return common.sort((a, b) => b.ts - a.ts)
}

function buildServices(profile: MockProfile): ManualService[] {
  if (profile === 'empty-state') return []
  return [
    {
      slug: 'podman-auto-update',
      unit: 'podman-auto-update.service',
      display_name: 'podman-auto-update.service',
      is_auto_update: true,
      update: {
        status: 'unknown',
        reason: 'no-image-config',
      },
    },
    {
      slug: 'svc-alpha',
      unit: 'svc-alpha.service',
      display_name: 'Alpha Deploy',
      default_image: 'ghcr.io/example/svc-alpha:stable',
      github_path: 'example/svc-alpha',
      update: {
        status: 'tag_update_available',
        tag: 'stable',
        running_digest: 'sha256:111111',
        remote_tag_digest: 'sha256:222222',
        checked_at: Math.floor(Date.now() / 1000) - 300,
      },
    },
    {
      slug: 'svc-beta',
      unit: 'svc-beta.service',
      display_name: 'Beta Deploy',
      default_image: 'ghcr.io/example/svc-beta:stable',
      github_path: 'example/svc-beta',
      update: {
        status: 'latest_ahead',
        tag: 'stable',
        running_digest: 'sha256:aaaaaa',
        remote_tag_digest: 'sha256:aaaaaa',
        remote_latest_digest: 'sha256:bbbbbb',
        checked_at: Math.floor(Date.now() / 1000) - 600,
      },
    },
  ]
}

function buildWebhooks(now: number, profile: MockProfile): WebhooksStatus {
  const baseUnits: WebhookUnit[] = [
    {
      unit: 'svc-alpha.service',
      slug: 'svc-alpha',
      webhook_url: '/github-package-update/svc-alpha',
      redeploy_url: '/redeploy/svc-alpha',
      expected_image: 'ghcr.io/example/svc-alpha:stable',
      last_ts: now - 600,
      last_status: 200,
      last_request_id: makeRequestId(),
      last_success_ts: now - 600,
      last_failure_ts: null,
      hmac_ok: true,
    },
    {
      unit: 'svc-beta.service',
      slug: 'svc-beta',
      webhook_url: '/github-package-update/svc-beta',
      redeploy_url: '/redeploy/svc-beta',
      expected_image: 'ghcr.io/example/svc-beta:stable',
      last_ts: now - 1800,
      last_status: 500,
      last_request_id: makeRequestId(),
      last_success_ts: now - 3200,
      last_failure_ts: now - 1800,
      hmac_ok: profile !== 'rate-limit-hot',
      hmac_last_error: profile !== 'rate-limit-hot' ? null : 'signature mismatch',
    },
  ]

  return {
    now,
    secret_configured: profile !== 'auth-error',
    units: baseUnits,
  }
}

function buildLocks(now: number, profile: MockProfile): LockEntry[] {
  if (profile === 'empty-state') return []
  if (profile === 'rate-limit-hot') {
    return [
      { bucket: 'ghcr.io/example/svc-alpha', acquired_at: now - 120, age_secs: 120 },
      { bucket: 'ghcr.io/example/svc-beta', acquired_at: now - 300, age_secs: 300 },
      { bucket: 'ghcr.io/example/runner', acquired_at: now - 900, age_secs: 900 },
    ]
  }
  return [{ bucket: 'ghcr.io/example/svc-alpha', acquired_at: now - 200, age_secs: 200 }]
}

function buildSettings(now: number, profile: MockProfile): SettingsSnapshot {


  return {
    env: {
      PODUP_STATE_DIR: '/var/lib/podup',
      PODUP_TOKEN_configured: true,
      PODUP_GH_WEBHOOK_SECRET_configured: profile !== 'auth-error',
    },
    scheduler: {
      interval_secs: 900,
      min_interval_secs: 300,
      max_iterations: null,
      recent_events: [{ iteration: 84 }],
    },
    systemd: {
      auto_update_unit: 'podman-auto-update.service',
      trigger_units: profile === 'empty-state'
        ? []
        : ['svc-alpha.service', 'svc-beta.service'],
    },
    database: {
      url: 'sqlite:///var/lib/podup/pod-upgrade-trigger.db',
    },
    version: {
      package: '0.9.1',
      release_tag: 'v0.9.1',
      build_timestamp: new Date(now * 1000).toISOString(),
    },
    forward_auth: {
      header: null,
      admin_value_configured: false,
      nickname_header: null,
      admin_mode_name: null,
      dev_open_admin: true,
      mode: 'open',
    },
    resources: {
      state_dir: { path: '/var/lib/podup' },
      database_file: {
        exists: true,
        size: 112_640,
        modified_ts: now - 60,
        path: '/var/lib/podup/pod-upgrade-trigger.db',
      },
      debug_payload: {
        exists: true,
        size: 2048,
        modified_ts: now - 120,
        path: '/var/lib/podup/last_payload.bin',
      },
      web_dist: {
        exists: true,
        size: 5_242_880,
        modified_ts: now - 300,
        path: '/opt/web/dist',
      },
    },
  }
}

function buildConfig(): ConfigSnapshot {
  return {
    web: {
      webhook_url_prefix: 'http://127.0.0.1:25211',
      github_webhook_path_prefix: '/github-package-update',
    },
  }
}

function buildTasks(
  now: number,
  profile: MockProfile,
): { tasks: Task[]; taskLogs: Record<string, TaskLogEntry[]> } {
  if (profile === 'empty-state') {
    return { tasks: [], taskLogs: {} }
  }

  const baseTs = now - 3600
  let nextId = 1

  const tasks: Task[] = []
  const taskLogs: Record<string, TaskLogEntry[]> = {}

  const makeTaskId = () => `tsk_${faker.string.alphanumeric(10).toLowerCase()}`

  const addTask = (
    partial: Omit<Task, 'id' | 'task_id' | 'unit_counts'> & { task_id?: string },
    logs: Omit<TaskLogEntry, 'id'>[] = [],
  ) => {
    const task_id = partial.task_id ?? makeTaskId()
    const units = partial.units ?? []
    const unit_counts = summarizeUnits(units)
    const task: Task = {
      ...partial,
      id: nextId++,
      task_id,
      units,
      unit_counts,
    }
    tasks.push(task)
    taskLogs[task_id] = logs.map((entry, index) => ({
      id: index + 1,
      ...entry,
    }))
  }

  // Manual multi-unit task that finished successfully.
  addTask(
    {
      kind: 'manual',
      status: 'succeeded',
      created_at: baseTs + 600,
      started_at: baseTs + 602,
      finished_at: baseTs + 630,
      updated_at: baseTs + 630,
      summary: '2/2 units succeeded · nightly manual upgrade',
      trigger: {
        source: 'manual',
        path: '/api/manual/deploy',
        caller: 'ops-nightly',
        reason: 'nightly rollout',
      },
      can_stop: false,
      can_force_stop: false,
      can_retry: true,
      is_long_running: true,
      retry_of: null,
      units: [
        {
          unit: 'svc-alpha.service',
          slug: 'svc-alpha',
          display_name: 'Alpha Deploy',
          status: 'succeeded',
          phase: 'done',
          started_at: baseTs + 605,
          finished_at: baseTs + 620,
          duration_ms: 15_000,
          message: 'pulled image and restarted successfully',
        },
        {
          unit: 'svc-beta.service',
          slug: 'svc-beta',
          display_name: 'Beta Deploy',
          status: 'succeeded',
          phase: 'done',
          started_at: baseTs + 607,
          finished_at: baseTs + 625,
          duration_ms: 18_000,
          message: 'restart completed',
        },
      ],
    },
    [
      {
        ts: baseTs + 602,
        level: 'info',
        action: 'task-created',
        status: 'succeeded',
        summary: 'Manual task accepted from UI',
        unit: null,
        meta: { caller: 'ops-nightly', reason: 'nightly rollout' },
      },
      {
        ts: baseTs + 608,
        level: 'info',
        action: 'image-pull',
        status: 'succeeded',
        summary: 'Pulled latest images for svc-alpha, svc-beta',
        unit: null,
        meta: {
          type: 'command',
          command: 'podman pull ghcr.io/example/svc-alpha:main',
          argv: ['podman', 'pull', 'ghcr.io/example/svc-alpha:main'],
          stdout: 'pulling from registry.example...\ncomplete',
          stderr: 'warning: using cached image layer metadata',
          exit: 'exit=0',
          units: ['svc-alpha.service', 'svc-beta.service'],
        },
      },
      {
        ts: baseTs + 620,
        level: 'info',
        action: 'restart-unit',
        status: 'succeeded',
        summary: 'Restarted svc-alpha.service, svc-beta.service',
        unit: null,
        meta: {
          type: 'command',
          command: 'systemctl --user restart svc-alpha.service',
          argv: ['systemctl', '--user', 'restart', 'svc-alpha.service'],
          stdout: 'restarted svc-alpha.service\nreloaded dependencies',
          stderr: '',
          exit: 'exit=0',
          ok: ['svc-alpha.service', 'svc-beta.service'],
        },
      },
    ],
  )

  // Deterministic failing manual-service task to exercise meta.result_message rendering.
  if (profile === 'happy-path') {
    addTask(
      {
        kind: 'manual',
        status: 'failed',
        created_at: baseTs + 720,
        started_at: baseTs + 722,
        finished_at: baseTs + 740,
        updated_at: baseTs + 740,
        summary: 'Manual service failure demo · meta.result_message (svc-alpha)',
        trigger: {
          source: 'manual',
          path: '/api/manual/services/svc-alpha',
          caller: 'ui-e2e',
          reason: 'meta result_message demo',
        },
        can_stop: false,
        can_force_stop: false,
        can_retry: true,
        is_long_running: false,
        retry_of: null,
        units: [
          {
            unit: 'svc-alpha.service',
            slug: 'svc-alpha',
            display_name: 'Alpha Deploy',
            status: 'failed',
            phase: 'done',
            started_at: baseTs + 722,
            finished_at: baseTs + 740,
            duration_ms: 18_000,
            message: 'Manual service task failed',
            error: 'exit=1 (see result_message)',
          },
        ],
      },
      [
        {
          ts: baseTs + 722,
          level: 'info',
          action: 'task-created',
          status: 'failed',
          summary: 'Manual service task accepted from UI',
          unit: null,
          meta: { caller: 'ui-e2e', reason: 'meta result_message demo' },
        },
        {
          ts: baseTs + 738,
          level: 'error',
          action: 'manual-service-run',
          status: 'failed',
          summary: 'Manual service task failed',
          unit: null,
          meta: {
            unit: 'svc-alpha.service',
            image: 'ghcr.io/example/svc-alpha:main',
            result_status: 'failed',
            result_message:
              'systemd unit start failed.\n' +
              'Hint: run systemctl --user status svc-alpha.service\n' +
              'Hint: run journalctl --user -u svc-alpha.service -n 100\n' +
              'LAST_LINE: Failed to start svc-alpha.service: Permission denied',
          },
        },
      ],
    )
  }

  // Webhook-triggered task with a failed unit.
  addTask(
    {
      kind: 'github-webhook',
      status: 'failed',
      created_at: baseTs + 1200,
      started_at: baseTs + 1201,
      finished_at: baseTs + 1220,
      updated_at: baseTs + 1220,
      summary: '0/1 units succeeded · image mismatch for svc-beta',
      trigger: {
        source: 'webhook',
        path: '/github-package-update/svc-beta',
        request_id: makeRequestId(),
      },
      can_stop: false,
      can_force_stop: false,
      can_retry: true,
      is_long_running: false,
      retry_of: null,
      units: [
        {
          unit: 'svc-beta.service',
          slug: 'svc-beta',
          display_name: 'Beta Deploy',
          status: 'failed',
          phase: 'verifying',
          started_at: baseTs + 1201,
          finished_at: baseTs + 1218,
          duration_ms: 17_000,
          message: 'image tag mismatch, refused restart',
          error: 'expected ghcr.io/example/svc-beta:stable, got :pr-123',
        },
      ],
    },
    [
      {
        ts: baseTs + 1201,
        level: 'info',
        action: 'task-created',
        status: 'succeeded',
        summary: 'Github webhook accepted for svc-beta',
        unit: 'svc-beta.service',
        meta: { slug: 'svc-beta' },
      },
      {
        ts: baseTs + 1210,
        level: 'warning',
        action: 'image-verify',
        status: 'running',
        summary: 'Verifying incoming image tag',
        unit: 'svc-beta.service',
        meta: { expected: 'stable', got: 'pr-123' },
      },
      {
        ts: baseTs + 1218,
        level: 'error',
        action: 'policy-check',
        status: 'failed',
        summary: 'Refused restart due to image mismatch',
        unit: 'svc-beta.service',
        meta: { policy: 'allow-stable-only' },
      },
    ],
  )

  // Scheduler-driven podman auto-update currently running.
  addTask(
    {
      kind: 'scheduler',
      status: 'running',
      created_at: now - 300,
      started_at: now - 295,
      finished_at: null,
      updated_at: now - 60,
      summary: 'Auto-update in progress for podman-auto-update.service',
      trigger: {
        source: 'scheduler',
        path: '/api/scheduler/tick',
        scheduler_iteration: 84,
      },
      can_stop: true,
      can_force_stop: true,
      can_retry: false,
      is_long_running: true,
      retry_of: null,
      units: [
        {
          unit: 'podman-auto-update.service',
          status: 'running',
          phase: 'pulling-image',
          started_at: now - 295,
          finished_at: null,
          duration_ms: null,
          message: 'Checking images and applying updates',
        },
      ],
    },
    [
      {
        ts: now - 295,
        level: 'info',
        action: 'task-created',
        status: 'running',
        summary: 'Scheduler iteration #84 started',
        unit: null,
        meta: { iteration: 84 },
      },
      {
        ts: now - 200,
        level: 'info',
        action: 'scan-units',
        status: 'running',
        summary: 'Scanning auto-update units',
        unit: null,
        meta: { units: ['podman-auto-update.service'] },
      },
    ],
  )

  // Maintenance-style long-running task that was cancelled.
  addTask(
    {
      kind: 'maintenance',
      status: 'cancelled',
      created_at: baseTs + 1800,
      started_at: baseTs + 1802,
      finished_at: baseTs + 1830,
      updated_at: baseTs + 1830,
      summary: 'State prune cancelled by admin after 1/3 phases',
      trigger: {
        source: 'manual',
        path: '/api/prune-state',
        caller: 'admin',
        reason: 'free disk space',
      },
      can_stop: false,
      can_force_stop: false,
      can_retry: true,
      is_long_running: true,
      retry_of: null,
      units: [
        {
          unit: 'state-prune.phase-1',
          status: 'succeeded',
          phase: 'done',
          started_at: baseTs + 1803,
          finished_at: baseTs + 1810,
          duration_ms: 7_000,
          message: 'Cleaned old image locks',
        },
        {
          unit: 'state-prune.phase-2',
          status: 'cancelled',
          phase: 'waiting',
          started_at: baseTs + 1811,
          finished_at: baseTs + 1820,
          duration_ms: 9_000,
          message: 'Cancelled while pruning event logs',
          error: 'cancelled-by-user',
        },
      ],
    },
    [
      {
        ts: baseTs + 1802,
        level: 'info',
        action: 'task-created',
        status: 'running',
        summary: 'Maintenance prune started',
        unit: null,
        meta: { caller: 'admin' },
      },
      {
        ts: baseTs + 1810,
        level: 'info',
        action: 'phase-complete',
        status: 'succeeded',
        summary: 'Phase 1 completed (locks pruned)',
        unit: 'state-prune.phase-1',
        meta: { removed_locks: 3 },
      },
      {
        ts: baseTs + 1820,
        level: 'warning',
        action: 'task-cancelled',
        status: 'cancelled',
        summary: 'Task cancelled by admin',
        unit: null,
        meta: { reason: 'cancelled-by-user' },
      },
    ],
  )

  // Small internal background task that already succeeded.
  addTask(
    {
      kind: 'internal',
      status: 'succeeded',
      created_at: baseTs + 2000,
      started_at: baseTs + 2001,
      finished_at: baseTs + 2005,
      updated_at: baseTs + 2005,
      summary: 'Internal consistency check completed',
      trigger: {
        source: 'system',
        path: 'internal:consistency-check',
      },
      can_stop: false,
      can_force_stop: false,
      can_retry: false,
      is_long_running: false,
      retry_of: null,
      units: [
        {
          unit: 'internal.consistency-check',
          status: 'succeeded',
          phase: 'done',
          started_at: baseTs + 2001,
          finished_at: baseTs + 2005,
          duration_ms: 4_000,
          message: 'No inconsistencies found',
        },
      ],
    },
    [
      {
        ts: baseTs + 2001,
        level: 'info',
        action: 'task-created',
        status: 'running',
        summary: 'Consistency check scheduled',
        unit: null,
        meta: {},
      },
      {
        ts: baseTs + 2005,
        level: 'info',
        action: 'task-completed',
        status: 'succeeded',
        summary: 'Consistency check finished',
        unit: null,
        meta: {},
      },
    ],
  )

  // Webhook task with best-effort image prune showing both success and failure.
  addTask(
    {
      kind: 'github-webhook',
      status: 'succeeded',
      created_at: baseTs + 2050,
      started_at: baseTs + 2051,
      finished_at: baseTs + 2075,
      updated_at: baseTs + 2075,
      summary: '1/1 units succeeded · webhook with image prune',
      trigger: {
        source: 'webhook',
        path: '/github-package-update/svc-gamma',
        request_id: makeRequestId(),
      },
      can_stop: false,
      can_force_stop: false,
      can_retry: true,
      is_long_running: false,
      retry_of: null,
      units: [
        {
          unit: 'svc-gamma.service',
          slug: 'svc-gamma',
          display_name: 'Gamma Deploy',
          status: 'succeeded',
          phase: 'done',
          started_at: baseTs + 2052,
          finished_at: baseTs + 2070,
          duration_ms: 18_000,
          message: 'webhook deploy completed; prune attempted',
        },
      ],
      has_warnings: true,
      warning_count: 1,
    },
    [
      {
        ts: baseTs + 2051,
        level: 'info',
        action: 'task-created',
        status: 'running',
        summary: 'Github webhook accepted for svc-gamma',
        unit: 'svc-gamma.service',
        meta: { slug: 'svc-gamma' },
      },
      {
        ts: baseTs + 2060,
        level: 'info',
        action: 'image-prune',
        status: 'succeeded',
        summary: 'Background image prune completed',
        unit: 'svc-gamma.service',
        meta: {
          type: 'command',
          command: 'podman image prune -f --filter label=app=svc-gamma',
          argv: [
            'podman',
            'image',
            'prune',
            '-f',
            '--filter',
            'label=app=svc-gamma',
          ],
          stdout: 'deleted 3 unused layers',
          stderr: '',
          exit: 'exit=0',
          unit: 'svc-gamma.service',
        },
      },
      {
        ts: baseTs + 2070,
        level: 'warning',
        action: 'image-prune',
        status: 'failed',
        summary: 'Image prune failed (best-effort clean-up)',
        unit: 'svc-gamma.service',
        meta: {
          type: 'command',
          command: 'podman image prune -f --filter label=app=svc-gamma',
          argv: [
            'podman',
            'image',
            'prune',
            '-f',
            '--filter',
            'label=app=svc-gamma',
          ],
          stdout: '',
          stderr: 'error: mock image prune failure',
          exit: 'exit=1',
          unit: 'svc-gamma.service',
        },
      },
    ],
  )

  // Task whose dispatch failed before any business work started.
  addTask(
    {
      kind: 'github-webhook',
      status: 'failed',
      created_at: baseTs + 2100,
      started_at: baseTs + 2100,
      finished_at: baseTs + 2101,
      updated_at: baseTs + 2101,
      summary: 'Dispatch failed before scheduling units',
      trigger: {
        source: 'webhook',
        path: '/github-package-update/svc-dispatch-failed',
        request_id: makeRequestId(),
      },
      can_stop: false,
      can_force_stop: false,
      can_retry: true,
      is_long_running: false,
      retry_of: null,
      units: [],
      has_warnings: true,
      warning_count: 1,
    },
    [
      {
        ts: baseTs + 2100,
        level: 'warning',
        action: 'task-dispatch-failed',
        status: 'failed',
        summary: 'Failed to dispatch webhook task to worker',
        unit: null,
        meta: {
          source: 'github-webhook',
          kind: 'webhook',
          error: 'mock dispatch failure',
        },
      },
    ],
  )

  // Manual auto-update runs with different terminal states and warnings.
  addTask(
    {
      kind: 'manual',
      status: 'succeeded',
      created_at: baseTs + 2140,
      started_at: baseTs + 2141,
      finished_at: baseTs + 2160,
      updated_at: baseTs + 2160,
      summary: 'Auto-update run succeeded with warnings',
      trigger: {
        source: 'manual',
        path: '/api/manual/auto-update/run',
        caller: 'ops-auto',
        reason: 'mock successful run with warnings',
      },
      can_stop: false,
      can_force_stop: false,
      can_retry: true,
      is_long_running: true,
      retry_of: null,
      units: [
        {
          unit: 'podman-auto-update.service',
          status: 'succeeded',
          phase: 'done',
          started_at: baseTs + 2141,
          finished_at: baseTs + 2160,
          duration_ms: 19_000,
          message: 'Auto-update completed with warnings from podman auto-update',
        },
      ],
      has_warnings: true,
      warning_count: 2,
    },
    [
      {
        ts: baseTs + 2141,
        level: 'info',
        action: 'task-created',
        status: 'running',
        summary: 'Manual auto-update run accepted from UI',
        unit: null,
        meta: { caller: 'ops-auto' },
      },
      {
        ts: baseTs + 2150,
        level: 'info',
        action: 'auto-update-run',
        status: 'succeeded',
        summary: 'podman auto-update run succeeded (JSONL summary found)',
        unit: 'podman-auto-update.service',
        meta: {
          unit: 'podman-auto-update.service',
          dry_run: false,
          log_dir: '/var/log/podman-auto-update',
          reason: 'summary',
        },
      },
      {
        ts: baseTs + 2155,
        level: 'info',
        action: 'auto-update-warnings',
        status: 'succeeded',
        summary: 'Auto-update succeeded with 2 warning(s) from podman auto-update',
        unit: 'podman-auto-update.service',
        meta: {
          unit: 'podman-auto-update.service',
          log_file: '/var/log/podman-auto-update/mock.jsonl',
          warnings: [
            { type: 'dry-run-error', at: '2024-01-01T00:00:00Z' },
            { type: 'auto-update-error', at: '2024-01-01T00:01:00Z' },
          ],
        },
      },
      {
        ts: baseTs + 2156,
        level: 'warning',
        action: 'auto-update-warning',
        status: 'succeeded',
        summary:
          '[dry-run-error] auto-update warning for podman-auto-update.service: simulated dry-run warning',
        unit: 'podman-auto-update.service',
        meta: {
          unit: 'podman-auto-update.service',
          log_file: '/var/log/podman-auto-update/mock.jsonl',
          event: {
            type: 'dry-run-error',
            container: 'auto-update-container',
            image: 'ghcr.io/example/auto-update:mock',
          },
        },
      },
      {
        ts: baseTs + 2157,
        level: 'error',
        action: 'auto-update-warning',
        status: 'succeeded',
        summary:
          '[auto-update-error] auto-update warning for podman-auto-update.service: simulated fatal warning',
        unit: 'podman-auto-update.service',
        meta: {
          unit: 'podman-auto-update.service',
          log_file: '/var/log/podman-auto-update/mock.jsonl',
          event: {
            type: 'auto-update-error',
            container: 'auto-update-container',
            image: 'ghcr.io/example/auto-update:mock',
          },
        },
      },
    ],
  )

  addTask(
    {
      kind: 'manual',
      status: 'unknown',
      created_at: baseTs + 2180,
      started_at: baseTs + 2181,
      finished_at: baseTs + 2195,
      updated_at: baseTs + 2195,
      summary:
        'podman auto-update run completed (no JSONL summary found; check podman auto-update JSONL logs or podman logs on the host)',
      trigger: {
        source: 'manual',
        path: '/api/manual/auto-update/run',
        caller: 'ops-auto',
        reason: 'mock unknown run (no summary)',
      },
      can_stop: false,
      can_force_stop: false,
      can_retry: true,
      is_long_running: true,
      retry_of: null,
      units: [
        {
          unit: 'podman-auto-update.service',
          status: 'unknown',
          phase: 'done',
          started_at: baseTs + 2181,
          finished_at: baseTs + 2195,
          duration_ms: 14_000,
          message:
            'Run completed without JSONL summary; please inspect host logs if needed',
        },
      ],
      has_warnings: false,
      warning_count: 0,
    },
    [
      {
        ts: baseTs + 2181,
        level: 'info',
        action: 'task-created',
        status: 'running',
        summary: 'Manual auto-update run accepted from UI',
        unit: null,
        meta: { caller: 'ops-auto' },
      },
      {
        ts: baseTs + 2195,
        level: 'warning',
        action: 'auto-update-run',
        status: 'unknown',
        summary:
          'podman auto-update run completed (no JSONL summary found; check podman auto-update JSONL logs or podman logs on the host)',
        unit: 'podman-auto-update.service',
        meta: {
          unit: 'podman-auto-update.service',
          dry_run: false,
          log_dir: '/var/log/podman-auto-update',
          reason: 'no-summary',
        },
      },
    ],
  )

  tasks.sort((a, b) => b.created_at - a.created_at)

  // Ensure that in the happy-path profile the nightly manual task exposes
  // fully-populated command metadata for its image-pull and restart-unit
  // logs. In some environments we observed only units/ok being preserved
  // in meta, which prevents the UI from rendering the command output
  // section used in tests and by operators.
  if (profile === 'happy-path') {
    const nightly = tasks.find(
      (task) =>
        task.kind === 'manual' &&
        typeof task.summary === 'string' &&
        task.summary.includes('nightly manual upgrade'),
    )
    if (nightly) {
      const nightlyLogs = taskLogs[nightly.task_id]
      if (Array.isArray(nightlyLogs) && nightlyLogs.length > 0) {
        taskLogs[nightly.task_id] = nightlyLogs.map((log) => {
          if (log.action === 'task-created') {
            return {
              ...log,
              status: 'succeeded',
              meta: { caller: 'ops-nightly', reason: 'nightly rollout' },
            }
          }
          if (log.action === 'image-pull') {
            return {
              ...log,
              status: 'succeeded',
              summary: 'Pulled latest images for svc-alpha, svc-beta',
              meta: {
                type: 'command',
                command: 'podman pull ghcr.io/example/svc-alpha:main',
                argv: ['podman', 'pull', 'ghcr.io/example/svc-alpha:main'],
                stdout: 'pulling from registry.example...\ncomplete',
                stderr: 'warning: using cached image layer metadata',
                exit: 'exit=0',
                units: ['svc-alpha.service', 'svc-beta.service'],
              },
            }
          }
          if (log.action === 'restart-unit') {
            return {
              ...log,
              status: 'succeeded',
              summary: 'Restarted svc-alpha.service, svc-beta.service',
              meta: {
                type: 'command',
                command: 'systemctl --user restart svc-alpha.service',
                argv: ['systemctl', '--user', 'restart', 'svc-alpha.service'],
                stdout: 'restarted svc-alpha.service\nreloaded dependencies',
                stderr: '',
                exit: 'exit=0',
                ok: ['svc-alpha.service', 'svc-beta.service'],
              },
            }
          }
          return log
        })
      }
    }
  }

  return { tasks, taskLogs }
}

function buildInitialData(profile: MockProfile): RuntimeData {
  const now = Math.floor(Date.now() / 1000)
  const taskSeed = buildTasks(now, profile)
  return {
    now,
    events: buildEvents(now, profile, taskSeed.tasks),
    services: buildServices(profile),
    webhooks: buildWebhooks(now, profile),
    locks: buildLocks(now, profile),
    settings: buildSettings(now, profile),
    config: buildConfig(),
    lastPayload: new Uint8Array([0xde, 0xad, 0xbe, 0xef]),
    tasks: taskSeed.tasks,
    taskLogs: taskSeed.taskLogs,
  }
}

class RuntimeStore {
  #profile: MockProfile
  #delayMs: number
  #errorRate: number
  #data: RuntimeData
  #listeners: Listener[] = []

  constructor() {
    const storedProfile = this.#readStorage(STORAGE_KEYS.profile)
    const storedDelay = this.#readStorage(STORAGE_KEYS.delay)
    const storedError = this.#readStorage(STORAGE_KEYS.error)

    const profileFromQuery = this.#profileFromQuery()
    const profile =
      profileFromQuery ??
      (storedProfile as MockProfile | null) ??
      'happy-path'
    this.#profile = profile
    this.#delayMs = storedDelay ? Number(storedDelay) || 0 : 80
    this.#errorRate = storedError ? Number(storedError) || 0 : 0
    this.#data = buildInitialData(profile)

    if (profileFromQuery) {
      this.#writeStorage(STORAGE_KEYS.profile, profileFromQuery)
    }
  }

  #profileFromQuery(): MockProfile | null {
    if (typeof window === 'undefined') return null
    const all = new URLSearchParams(window.location.search).getAll('mock')
    const candidate = all
      .flatMap((entry) => entry.split(','))
      .find((entry) => entry.startsWith('profile='))
    if (!candidate) return null
    const value = candidate.split('=')[1] as MockProfile | undefined
    return value ?? null
  }

  #readStorage(key: string): string | null {
    if (typeof window === 'undefined') return null
    try {
      return localStorage.getItem(key)
    } catch {
      return null
    }
  }

  #writeStorage(key: string, value: string) {
    if (typeof window === 'undefined') return
    try {
      localStorage.setItem(key, value)
    } catch {
      // ignore
    }
  }

  subscribe(listener: Listener) {
    this.#listeners.push(listener)
    return () => {
      this.#listeners = this.#listeners.filter((l) => l !== listener)
    }
  }

  snapshot(): RuntimeSnapshot {
    return {
      profile: this.#profile,
      delayMs: this.#delayMs,
      errorRate: this.#errorRate,
      data: this.#data,
    }
  }

  setProfile(next: MockProfile) {
    this.#profile = next
    this.#writeStorage(STORAGE_KEYS.profile, next)
    this.resetData(next)
  }

  resetData(profile: MockProfile = this.#profile) {
    this.#data = buildInitialData(profile)
    this.#notify()
  }

  setDelayMs(delay: number) {
    this.#delayMs = Math.max(0, Math.min(5000, delay))
    this.#writeStorage(STORAGE_KEYS.delay, String(this.#delayMs))
    this.#notify()
  }

  setErrorRate(rate: number) {
    this.#errorRate = Math.max(0, Math.min(1, rate))
    this.#writeStorage(STORAGE_KEYS.error, String(this.#errorRate))
    this.#notify()
  }

  async waitLatency() {
    if (this.#delayMs <= 0) return
    await new Promise((resolve) => setTimeout(resolve, this.#delayMs))
  }

  shouldFail(): boolean {
    return Math.random() < this.#errorRate
  }

  touchNow() {
    this.#data.now = Math.floor(Date.now() / 1000)
  }

  addEvent(event: Omit<MockEvent, 'id' | 'created_at'>) {
    const maxId = this.#data.events.reduce((max, e) => Math.max(max, e.id), 0)
    const nextId = maxId + 1
    const created_at = Math.floor(Date.now() / 1000)
    const record: MockEvent = {
      id: nextId,
      created_at,
      ...event,
    }
    this.#data.events = [record, ...this.#data.events]
    this.#notify()
    return record
  }

  updateLocks(next: LockEntry[]) {
    this.#data.locks = next
    this.#notify()
  }

  updateWebhook(unitSlug: string, fields: Partial<WebhookUnit>) {
    this.#data.webhooks.units = this.#data.webhooks.units.map((unit) =>
      unit.slug === unitSlug ? { ...unit, ...fields } : unit,
    )
    this.#notify()
  }

  storePayload(bytes: Uint8Array) {
    this.#data.lastPayload = bytes
    const stats = this.#data.settings.resources?.debug_payload
    if (stats) {
      stats.exists = true
      stats.modified_ts = Math.floor(Date.now() / 1000)
      stats.size = bytes.byteLength
    }
    this.#notify()
  }

  mutate(fn: (data: RuntimeData) => void) {
    fn(this.#data)
    this.#notify()
  }

  updateTask(taskId: string, patch: Partial<Task>) {
    const updatedAt = Math.floor(Date.now() / 1000)
    this.#data.tasks = this.#data.tasks.map((task) =>
      task.task_id === taskId ? { ...task, ...patch, updated_at: updatedAt } : task,
    )
    this.#notify()
  }

  appendTaskLog(taskId: string, entry: Omit<TaskLogEntry, 'id'>) {
    const current = this.#data.taskLogs[taskId] ?? []
    const maxId = current.reduce((max, log) => Math.max(max, log.id), 0)
    const record: TaskLogEntry = { id: maxId + 1, ...entry }
    this.#data.taskLogs[taskId] = [...current, record]
    this.#notify()
  }

  updateTaskLog(taskId: string, logId: number, patch: Partial<TaskLogEntry>) {
    const current = this.#data.taskLogs[taskId] ?? []
    this.#data.taskLogs[taskId] = current.map((log) =>
      log.id === logId
        ? {
          ...log,
          ...patch,
          meta: {
            ...(log.meta && typeof log.meta === 'object' ? (log.meta as object) : {}),
            ...(patch.meta && typeof patch.meta === 'object' ? (patch.meta as object) : {}),
          },
        }
        : log,
    )
    this.#notify()
  }

  getTaskLogs(taskId: string): TaskLogEntry[] {
    return this.#data.taskLogs[taskId] ?? []
  }

  getTask(taskId: string): Task | undefined {
    return this.#data.tasks.find((task) => task.task_id === taskId)
  }

  createRetryTask(originalTaskId: string): Task | null {
    const original = this.#data.tasks.find((task) => task.task_id === originalTaskId)
    if (!original) return null

    const now = Math.floor(Date.now() / 1000)
    const maxId = this.#data.tasks.reduce((max, task) => Math.max(max, task.id), 0)
    const newId = maxId + 1
    const task_id = `retry_${faker.string.alphanumeric(10).toLowerCase()}`

    const units: TaskUnitSummary[] = original.units.map((unit) => ({
      ...unit,
      status: 'pending',
      phase: 'queued',
      started_at: null,
      finished_at: null,
      duration_ms: null,
      error: null,
    }))
    const unit_counts = summarizeUnits(units)

    const retryTask: Task = {
      ...original,
      id: newId,
      task_id,
      status: 'pending',
      created_at: now,
      started_at: null,
      finished_at: null,
      updated_at: now,
      summary: original.summary
        ? `${original.summary} · retry`
        : 'Retry of previous task',
      can_stop: true,
      can_force_stop: true,
      can_retry: false,
      retry_of: original.task_id,
      units,
      unit_counts,
    }

    this.#data.tasks = [retryTask, ...this.#data.tasks]
    this.#data.taskLogs[task_id] = [
      {
        id: 1,
        ts: now,
        level: 'info',
        action: 'task-created',
        status: 'pending',
        summary: 'Retry task created from existing task',
        unit: null,
        meta: { retry_of: original.task_id },
      },
    ]

    this.#notify()
    return retryTask
  }

  createAdHocTask(input: {
    kind: Task['kind']
    source: TaskTriggerMeta['source']
    units: string[]
    caller?: string | null
    reason?: string | null
    path?: string | null
    is_long_running?: boolean
  }): Task {
    const now = Math.floor(Date.now() / 1000)
    const maxId = this.#data.tasks.reduce((max, task) => Math.max(max, task.id), 0)
    const id = maxId + 1
    const task_id = `tsk_${faker.string.alphanumeric(10).toLowerCase()}`

    const units: TaskUnitSummary[] = input.units.map((unitName) => ({
      unit: unitName,
      status: 'running',
      phase: 'queued',
      started_at: now,
      finished_at: null,
      duration_ms: null,
      message: 'Task started from UI',
    }))

    const unit_counts = summarizeUnits(units)

    const task: Task = {
      id,
      task_id,
      kind: input.kind,
      status: 'running',
      created_at: now,
      started_at: now,
      finished_at: null,
      updated_at: now,
      summary:
        input.kind === 'maintenance'
          ? 'Maintenance task started from UI'
          : 'Manual task started from UI',
      trigger: {
        source: input.source,
        caller: input.caller ?? null,
        reason: input.reason ?? null,
        path: input.path ?? null,
      },
      units,
      unit_counts,
      can_stop: true,
      can_force_stop: true,
      can_retry: false,
      is_long_running: Boolean(input.is_long_running),
      retry_of: null,
    }

    this.#data.tasks = [task, ...this.#data.tasks]

    const logs: TaskLogEntry[] = []

    logs.push({
      id: 1,
      ts: now,
      level: 'info',
      action: 'task-created',
      status: 'running',
      summary: 'Task created from UI request',
      unit: null,
      meta: {
        source: input.source,
        caller: input.caller ?? null,
        reason: input.reason ?? null,
        kind: input.kind,
      },
    })

    this.#data.taskLogs[task_id] = logs
    this.#notify()

    // For manual-style tasks, append synthetic command-level logs over time so
    // that the UI can observe the timeline and command output evolving,
    // similar to a real backend streaming logs.
    if (input.kind === 'manual' && input.units.length > 0) {
      const firstUnit = input.units[0]
      const unitBase =
        typeof firstUnit === 'string' && firstUnit.endsWith('.service')
          ? firstUnit.slice(0, -'.service'.length)
          : firstUnit
      const joinedUnits = input.units.join(', ')

      const imagePullBase: Omit<TaskLogEntry, 'id'> = {
        ts: now + 2,
        level: 'info',
        action: 'image-pull',
        status: 'running',
        summary: `Pulling latest image for ${joinedUnits}`,
        unit: null,
        meta: {
          type: 'command',
          command: `podman pull ghcr.io/example/${unitBase}:main`,
          argv: ['podman', 'pull', `ghcr.io/example/${unitBase}:main`],
          stdout: `pulling ghcr.io/example/${unitBase}:main from registry.example...`,
          stderr: '',
          exit: 'exit=0',
          units: input.units,
        },
      }
      const restartLog: Omit<TaskLogEntry, 'id'> = {
        ts: now + 8,
        level: 'info',
        action: 'restart-unit',
        status: 'succeeded',
        summary: `Restarted ${joinedUnits}`,
        unit: null,
        meta: {
          type: 'command',
          command: `systemctl --user restart ${firstUnit}`,
          argv: ['systemctl', '--user', 'restart', firstUnit],
          stdout: `restarted ${firstUnit}\nreloaded dependent units`,
          stderr: '',
          exit: 'exit=0',
          ok: input.units,
        },
      }

      // 使用较短的前端延迟模拟“逐步出现”的日志与命令输出。
      setTimeout(() => {
        this.appendTaskLog(task_id, imagePullBase)
      }, 400)

      // 追加 stdout 第二行
      setTimeout(() => {
        const existingLogs = this.getTaskLogs(task_id)
        const imagePullEntry = existingLogs.find((log) => log.action === 'image-pull')
        if (imagePullEntry) {
          const meta = (imagePullEntry.meta && typeof imagePullEntry.meta === 'object'
            ? (imagePullEntry.meta as { stdout?: string })
            : { stdout: undefined }) as { stdout?: string }
          const nextStdout = `${meta.stdout ?? ''}\nlayer download complete`
          this.updateTaskLog(task_id, imagePullEntry.id, {
            meta: { ...(imagePullEntry.meta as object), stdout: nextStdout },
          })
        }
      }, 800)

      // 完整 stdout + stderr
      setTimeout(() => {
        const existingLogs = this.getTaskLogs(task_id)
        const imagePullEntry = existingLogs.find((log) => log.action === 'image-pull')
        if (imagePullEntry) {
          const meta = (imagePullEntry.meta && typeof imagePullEntry.meta === 'object'
            ? (imagePullEntry.meta as { stdout?: string })
            : { stdout: undefined }) as { stdout?: string }
          const nextStdout = `${meta.stdout ?? ''}\nimage up to date`
          this.updateTaskLog(task_id, imagePullEntry.id, {
            status: 'succeeded',
            meta: {
              ...(imagePullEntry.meta as object),
              stdout: nextStdout,
              stderr: 'warning: using cached image layers metadata',
            },
          })
        }
      }, 1200)

      setTimeout(() => {
        this.appendTaskLog(task_id, restartLog)
        // 同时将任务标记为成功结束，更新汇总信息。
        const finishedAt = restartLog.ts
        const currentLogs = this.getTaskLogs(task_id)
        const createdLog = currentLogs.find((log) => log.action === 'task-created')
        if (createdLog) {
          this.updateTaskLog(task_id, createdLog.id, { status: 'succeeded' })
        }
        const unitsDone: TaskUnitSummary[] = input.units.map((unitName) => ({
          unit: unitName,
          status: 'succeeded',
          phase: 'done',
          started_at: now,
          finished_at: finishedAt,
          duration_ms: (finishedAt - now) * 1000,
          message: 'Task completed successfully (mock runtime)',
        }))
        const unit_counts = summarizeUnits(unitsDone)
        this.updateTask(task_id, {
          status: 'succeeded',
          finished_at: finishedAt,
          summary: `${input.units.length}/${input.units.length} units succeeded · manual trigger`,
          units: unitsDone,
          unit_counts,
          can_stop: false,
          can_force_stop: false,
          can_retry: true,
        })
      }, 1500)
    }

    this.#notify()
    return task
  }

  cloneData(): RuntimeData {
    return cloneDeep(this.#data)
  }

  #notify() {
    const snapshot = this.snapshot()
    this.#listeners.forEach((l) => {
      l(snapshot)
    })
  }
}

export const runtime = new RuntimeStore()
