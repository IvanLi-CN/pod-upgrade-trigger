import { http, HttpResponse } from 'msw'
import type { TaskDetailResponse, TasksListResponse } from '../domain/tasks'
import { runtime } from './runtime'
import {
  settingsSchema,
  webhooksStatusSchema,
  tasksListResponseSchema,
  taskDetailResponseSchema,
  validateMockResponse,
} from './schema'

const JSON_HEADERS = { 'Content-Type': 'application/json' }

function makeRequestId() {
  return Math.random().toString(16).slice(2, 12)
}

function shouldAuthFail(pathname: string) {
  if (runtime.snapshot().profile !== 'auth-error') return false
  // allow health checks to surface degraded separately
  return pathname.startsWith('/api') || pathname.startsWith('/github-package-update')
}

async function withLatency() {
  runtime.touchNow()
  const latency = runtime.snapshot().delayMs
  if (latency > 0) {
    await runtime.waitLatency()
  }
}

function maybeFailure() {
  if (runtime.shouldFail()) {
    return HttpResponse.json({ error: 'mocked error' }, { status: 500 })
  }
  return null
}

function isManualForcedFailure() {
  try {
    return Boolean(
      (window as unknown as { __MOCK_FORCE_MANUAL_FAILURE__?: boolean })
        .__MOCK_FORCE_MANUAL_FAILURE__,
    )
  } catch {
    return false
  }
}

function authGuard(url: URL) {
  if (!shouldAuthFail(url.pathname)) return null
  return HttpResponse.json({ error: 'unauthorized in mock profile' }, { status: 401 })
}

function degradedGuard(url: URL) {
  if (runtime.snapshot().profile !== 'degraded') return null
  if (url.pathname === '/health') return HttpResponse.json({ status: 'fail' }, { status: 503 })
  if (url.pathname === '/sse/hello') return HttpResponse.json({ error: 'sse down' }, { status: 503 })
  return null
}

const handlers = [
  http.get('/health', async ({ request }) => {
    const url = new URL(request.url)
    const failure = degradedGuard(url) || authGuard(url) || maybeFailure()
    if (failure) return failure
    await withLatency()
    return HttpResponse.json({ status: 'ok' }, { status: 200 })
  }),

  http.get('/sse/hello', async ({ request }) => {
    const url = new URL(request.url)
    const failure = degradedGuard(url) || authGuard(url) || maybeFailure()
    if (failure) return failure

    await withLatency()

    const stream = new ReadableStream({
      start(controller) {
        const encoder = new TextEncoder()
        controller.enqueue(encoder.encode('event: hello\\ndata: ok\\n\\n'))
        controller.close()
      },
    })

    return new HttpResponse(stream, {
      status: 200,
      headers: {
        'Content-Type': 'text/event-stream',
        Connection: 'keep-alive',
        'Cache-Control': 'no-cache',
      },
    })
  }),

  http.get('/api/settings', async ({ request }) => {
    const url = new URL(request.url)
    const failure = degradedGuard(url) || authGuard(url) || maybeFailure()
    if (failure) return failure
    await withLatency()
    const payload = runtime.cloneData().settings
    validateMockResponse(settingsSchema, payload, { path: '/api/settings' })
    return HttpResponse.json(payload, { headers: JSON_HEADERS })
  }),

  http.get('/api/events', async ({ request }) => {
    const url = new URL(request.url)
    const failure = degradedGuard(url) || authGuard(url) || maybeFailure()
    if (failure) return failure
    await withLatency()

    const params = url.searchParams
    const perPage = Number(params.get('per_page') ?? params.get('limit')) || 50
    const page = Number(params.get('page')) || 1
    const requestId = params.get('request_id') || ''
    const pathPrefix = params.get('path_prefix') || ''
    const status = params.get('status') || ''
    const action = params.get('action') || ''

    let events = runtime.cloneData().events
    if (requestId) events = events.filter((e) => e.request_id.includes(requestId))
    if (pathPrefix) events = events.filter((e) => (e.path ?? '').startsWith(pathPrefix))
    if (status) events = events.filter((e) => String(e.status).startsWith(status))
    if (action) events = events.filter((e) => e.action.startsWith(action))

    const total = events.length
    const start = (page - 1) * perPage
    const slice = events.slice(start, start + perPage)
    const hasNext = start + perPage < total

    const response = {
      events: slice,
      total,
      page,
      page_size: perPage,
      has_next: hasNext,
    }

    return HttpResponse.json(response, { headers: JSON_HEADERS })
  }),

  http.get('/api/tasks', async ({ request }) => {
    const url = new URL(request.url)
    const failure = degradedGuard(url) || authGuard(url) || maybeFailure()
    if (failure) return failure
    await withLatency()

    const params = url.searchParams
    const perPage = Number(params.get('per_page') ?? params.get('limit')) || 20
    const page = Number(params.get('page')) || 1
    const statusFilter = params.get('status') || ''
    const kindFilter = params.get('kind') || params.get('type') || ''
    const unitFilter = params.get('unit') || params.get('unit_query') || ''

    let tasks = runtime.cloneData().tasks

    if (statusFilter) {
      tasks = tasks.filter((task) => task.status === statusFilter)
    }
    if (kindFilter) {
      tasks = tasks.filter((task) => task.kind === kindFilter)
    }
    if (unitFilter) {
      const needle = unitFilter.toLowerCase()
      tasks = tasks.filter((task) =>
        task.units.some((unit) => {
          const parts = [
            unit.unit,
            unit.slug ?? '',
            unit.display_name ?? '',
          ]
          return parts.some((part) => part.toLowerCase().includes(needle))
        }),
      )
    }

    const total = tasks.length
    const start = (page - 1) * perPage
    const slice = tasks.slice(start, start + perPage)
    const hasNext = start + perPage < total

    const response: TasksListResponse = {
      tasks: slice,
      total,
      page,
      page_size: perPage,
      has_next: hasNext,
    }

    validateMockResponse(tasksListResponseSchema, response, { path: '/api/tasks' })

    return HttpResponse.json(response, { headers: JSON_HEADERS })
  }),

  http.get('/api/tasks/:id', async ({ params, request }) => {
    const url = new URL(request.url)
    const failure = degradedGuard(url) || authGuard(url) || maybeFailure()
    if (failure) return failure
    await withLatency()

    const taskId = String(params.id)
    const task = runtime.getTask(taskId)
    if (!task) {
      return HttpResponse.json({ error: 'task not found' }, { status: 404 })
    }

    const logs = runtime.getTaskLogs(taskId)
    const response: TaskDetailResponse = {
      ...task,
      logs,
    }

    validateMockResponse(taskDetailResponseSchema, response, {
      path: `/api/tasks/${taskId}`,
    })

    return HttpResponse.json(response, { headers: JSON_HEADERS })
  }),

  http.post('/api/tasks', async ({ request }) => {
    const url = new URL(request.url)
    const failure = degradedGuard(url) || authGuard(url) || maybeFailure()
    if (failure) return failure
    await withLatency()

    type CreateTaskBody = {
      kind?: string
      source?: string
      units?: string[]
      caller?: string | null
      reason?: string | null
      path?: string | null
      is_long_running?: boolean
    }

    const body = (await request.json().catch(() => ({}))) as CreateTaskBody
    const kind = (body.kind as TaskDetailResponse['kind'] | undefined) ?? 'manual'
    const source = (body.source as TaskDetailResponse['trigger']['source'] | undefined) ?? 'manual'
    const units = Array.isArray(body.units) && body.units.length > 0 ? body.units : ['unknown.unit']

    const task = runtime.createAdHocTask({
      kind,
      source,
      units,
      caller: body.caller ?? null,
      reason: body.reason ?? null,
      path: body.path ?? null,
      is_long_running: body.is_long_running ?? true,
    })

    return HttpResponse.json(
      {
        task_id: task.task_id,
        is_long_running: task.is_long_running ?? false,
        kind: task.kind,
        status: task.status,
      },
      { headers: JSON_HEADERS },
    )
  }),

  http.post('/api/tasks/:id/stop', async ({ params, request }) => {
    const url = new URL(request.url)
    const failure = degradedGuard(url) || authGuard(url) || maybeFailure()
    if (failure) return failure
    await withLatency()

    const taskId = String(params.id)
    const existing = runtime.getTask(taskId)
    if (!existing) {
      return HttpResponse.json({ error: 'task not found' }, { status: 404 })
    }

    const now = Math.floor(Date.now() / 1000)
    if (existing.status === 'running') {
      const summary = existing.summary?.includes('cancelled')
        ? existing.summary
        : `${existing.summary ?? 'Task'} · cancelled by user`

      runtime.updateTask(taskId, {
        status: 'cancelled',
        finished_at: existing.finished_at ?? now,
        summary,
        can_stop: false,
        can_force_stop: false,
      })
      runtime.appendTaskLog(taskId, {
        ts: now,
        level: 'warning',
        action: 'task-cancelled',
        status: 'cancelled',
        summary: 'Task cancelled via mock /stop endpoint',
        unit: null,
        meta: { via: 'stop' },
      })
    } else {
      runtime.appendTaskLog(taskId, {
        ts: now,
        level: 'info',
        action: 'task-stop-noop',
        status: existing.status,
        summary: 'Stop requested but task already in terminal state',
        unit: null,
        meta: { status: existing.status },
      })
    }

    const updated = runtime.getTask(taskId) ?? existing
    const logs = runtime.getTaskLogs(taskId)
    const response: TaskDetailResponse = {
      ...updated,
      logs,
    }

    return HttpResponse.json(response, { headers: JSON_HEADERS })
  }),

  http.post('/api/tasks/:id/force-stop', async ({ params, request }) => {
    const url = new URL(request.url)
    const failure = degradedGuard(url) || authGuard(url) || maybeFailure()
    if (failure) return failure
    await withLatency()

    const taskId = String(params.id)
    const existing = runtime.getTask(taskId)
    if (!existing) {
      return HttpResponse.json({ error: 'task not found' }, { status: 404 })
    }

    const now = Math.floor(Date.now() / 1000)
    if (existing.status === 'running') {
      const summary = existing.summary?.includes('force-stopped')
        ? existing.summary
        : `${existing.summary ?? 'Task'} · force-stopped`

      runtime.updateTask(taskId, {
        status: 'failed',
        finished_at: existing.finished_at ?? now,
        summary,
        can_stop: false,
        can_force_stop: false,
      })
      runtime.appendTaskLog(taskId, {
        ts: now,
        level: 'error',
        action: 'task-force-killed',
        status: 'failed',
        summary: 'Task force-stopped via mock /force-stop endpoint',
        unit: null,
        meta: { via: 'force-stop' },
      })
    } else {
      runtime.appendTaskLog(taskId, {
        ts: now,
        level: 'info',
        action: 'task-force-stop-noop',
        status: existing.status,
        summary: 'Force-stop requested but task already in terminal state',
        unit: null,
        meta: { status: existing.status },
      })
    }

    const updated = runtime.getTask(taskId) ?? existing
    const logs = runtime.getTaskLogs(taskId)
    const response: TaskDetailResponse = {
      ...updated,
      logs,
    }

    return HttpResponse.json(response, { headers: JSON_HEADERS })
  }),

  http.post('/api/tasks/:id/retry', async ({ params, request }) => {
    const url = new URL(request.url)
    const failure = degradedGuard(url) || authGuard(url) || maybeFailure()
    if (failure) return failure
    await withLatency()

    const taskId = String(params.id)
    const original = runtime.getTask(taskId)
    if (!original) {
      return HttpResponse.json({ error: 'task not found' }, { status: 404 })
    }

    if (original.status === 'running' || original.status === 'pending') {
      return HttpResponse.json(
        { error: 'cannot retry a running or pending task' },
        { status: 409 },
      )
    }

    const retryTask = runtime.createRetryTask(taskId)
    if (!retryTask) {
      return HttpResponse.json({ error: 'failed to create retry task' }, { status: 500 })
    }

    const now = Math.floor(Date.now() / 1000)
    runtime.appendTaskLog(taskId, {
      ts: now,
      level: 'info',
      action: 'task-retried',
      status: original.status,
      summary: 'Retry task created from this task',
      unit: null,
      meta: { retry_task_id: retryTask.task_id },
    })

    const logs = runtime.getTaskLogs(retryTask.task_id)
    const response: TaskDetailResponse = {
      ...retryTask,
      logs,
    }

    return HttpResponse.json(response, { headers: JSON_HEADERS })
  }),

  http.get('/api/manual/services', async ({ request }) => {
    const url = new URL(request.url)
    const failure = degradedGuard(url) || authGuard(url) || maybeFailure()
    if (failure) return failure
    await withLatency()
    const services = runtime.cloneData().services
    return HttpResponse.json({ services }, { headers: JSON_HEADERS })
  }),

  http.post('/api/manual/trigger', async ({ request }) => {
    const url = new URL(request.url)
    const failure = degradedGuard(url) || authGuard(url) || maybeFailure()
    if (failure) return failure
    if (isManualForcedFailure()) {
      return HttpResponse.json({ error: 'forced failure' }, { status: 500 })
    }
    await withLatency()

    const body = (await request.json().catch(() => ({}))) as Record<string, unknown>
    const services = runtime.cloneData().services
    const dryRun = Boolean(body.dry_run)

    const units = services.map((svc) => svc.unit)

    const triggered = units.map((unit) => ({
      unit,
      status: dryRun ? 'dry-run' : 'pending',
      message: dryRun ? 'ok' : 'scheduled via task',
    }))

    const caller =
      typeof body.caller === 'string' && body.caller.trim()
        ? body.caller.trim()
        : null
    const reason =
      typeof body.reason === 'string' && body.reason.trim()
        ? body.reason.trim()
        : null

    const requestId = runtime.addEvent({
      request_id: caller ?? `manual-${Date.now()}`,
      ts: Math.floor(Date.now() / 1000),
      method: 'POST',
      path: '/api/manual/trigger',
      status: 200,
      action: 'manual-trigger',
      duration_ms: 180,
      meta: body,
    }).request_id

    let taskId: string | null = null
    if (!dryRun) {
      const task = runtime.createAdHocTask({
        kind: 'manual',
        source: 'manual',
        units,
        caller,
        reason,
        path: '/api/manual/trigger',
        is_long_running: true,
      })
      taskId = task.task_id
    }

    return HttpResponse.json(
      {
        triggered,
        dry_run: dryRun,
        request_id: requestId,
        caller,
        reason,
        task_id: taskId,
      },
      { headers: JSON_HEADERS },
    )
  }),

  http.post('/api/manual/services/:slug', async ({ params, request }) => {
    const url = new URL(request.url)
    const failure = degradedGuard(url) || authGuard(url) || maybeFailure()
    if (failure) return failure
    await withLatency()

    const services = runtime.cloneData().services
    const service = services.find((s) => s.slug === params.slug)
    const body = (await request.json().catch(() => ({}))) as Record<string, unknown>

    if (!service) {
      return HttpResponse.json({ error: 'service not found' }, { status: 404 })
    }

    const dryRun = Boolean(body.dry_run)
    const status = dryRun ? 'dry-run' : 'pending'

    const caller =
      typeof body.caller === 'string' && body.caller.trim()
        ? body.caller.trim()
        : null
    const reason =
      typeof body.reason === 'string' && body.reason.trim()
        ? body.reason.trim()
        : null

    runtime.addEvent({
      request_id: makeRequestId(),
      ts: Math.floor(Date.now() / 1000),
      method: 'POST',
      path: `/api/manual/services/${service.slug}`,
      status: dryRun ? 202 : 202,
      action: 'manual-trigger',
      duration_ms: 140,
      meta: { service: service.slug, ...body },
    })

    let taskId: string | null = null
    if (!dryRun) {
      const task = runtime.createAdHocTask({
        kind: 'manual',
        source: 'manual',
        units: [service.unit],
        caller,
        reason,
        path: `/api/manual/services/${service.slug}`,
        is_long_running: true,
      })
      taskId = task.task_id
    }

    return HttpResponse.json(
      {
        unit: service.unit,
        status,
        request_id: makeRequestId(),
        caller,
        reason,
        task_id: taskId,
      },
      { headers: JSON_HEADERS },
    )
  }),

  http.get('/api/webhooks/status', async ({ request }) => {
    const url = new URL(request.url)
    const failure = degradedGuard(url) || authGuard(url) || maybeFailure()
    if (failure) return failure
    await withLatency()

    const data = runtime.cloneData().webhooks
    const now = Math.floor(Date.now() / 1000)
    const payload = { ...data, now }
    validateMockResponse(webhooksStatusSchema, payload, {
      path: '/api/webhooks/status',
    })
    return HttpResponse.json(payload, { headers: JSON_HEADERS })
  }),

  http.get('/api/image-locks', async ({ request }) => {
    const url = new URL(request.url)
    const failure = degradedGuard(url) || authGuard(url) || maybeFailure()
    if (failure) return failure
    await withLatency()

    const now = Math.floor(Date.now() / 1000)
    const locks = runtime.cloneData().locks.map((lock) => ({
      ...lock,
      age_secs: Math.max(0, now - lock.acquired_at),
    }))

    return HttpResponse.json({ now, locks }, { headers: JSON_HEADERS })
  }),

  http.delete('/api/image-locks/:bucket', async ({ params, request }) => {
    const url = new URL(request.url)
    const failure = degradedGuard(url) || authGuard(url) || maybeFailure()
    if (failure) return failure
    await withLatency()

    const bucket = decodeURIComponent(String(params.bucket))
    const nextLocks = runtime.cloneData().locks.filter((lock) => lock.bucket !== bucket)
    const removed = nextLocks.length !== runtime.cloneData().locks.length
    if (removed) {
      runtime.updateLocks(nextLocks)
    }

    return HttpResponse.json({ bucket, removed, rows: removed ? 1 : 0 }, { headers: JSON_HEADERS })
  }),

  http.get('/api/config', async ({ request }) => {
    const url = new URL(request.url)
    const failure = degradedGuard(url) || authGuard(url) || maybeFailure()
    if (failure) return failure
    await withLatency()
    return HttpResponse.json(runtime.cloneData().config, { headers: JSON_HEADERS })
  }),

  http.post('/api/prune-state', async ({ request }) => {
    const url = new URL(request.url)
    const failure = degradedGuard(url) || authGuard(url) || maybeFailure()
    if (failure) return failure
    await withLatency()

    const body = (await request.json().catch(() => ({}))) as Record<string, unknown>
    const maxAge = Number(body.max_age_hours) || 24
    const dryRun = Boolean(body.dry_run)

    runtime.updateLocks([])

    let taskId: string | null = null
    try {
      const task = runtime.createAdHocTask({
        kind: 'maintenance',
        source: 'maintenance',
        units: ['state-prune'],
        is_long_running: true,
      })
      taskId = task.task_id
    } catch {
      // If task creation fails in mock mode, still return prune result without task_id.
    }

    return HttpResponse.json(
      {
        tokens_removed: 4,
        locks_removed: 3,
        legacy_dirs_removed: 1,
        dry_run: dryRun,
        max_age_hours: maxAge,
        task_id: taskId,
      },
      { headers: JSON_HEADERS },
    )
  }),

  http.get('/last_payload.bin', async ({ request }) => {
    const url = new URL(request.url)
    const failure = degradedGuard(url) || maybeFailure()
    if (failure) return failure
    await withLatency()

    const buffer = runtime.cloneData().lastPayload
    return new HttpResponse(buffer, {
      status: 200,
      headers: {
        'Content-Type': 'application/octet-stream',
        'Content-Length': String(buffer.byteLength),
      },
    })
  }),

  http.post('/github-package-update/:slug', async ({ params, request }) => {
    const url = new URL(request.url)
    const failure = degradedGuard(url) || maybeFailure()
    if (failure) return failure
    await withLatency()

    const bytes = new Uint8Array(await request.arrayBuffer())
    runtime.storePayload(bytes)
    runtime.updateWebhook(String(params.slug), {
      hmac_ok: false,
      hmac_last_error: 'signature mismatch',
      last_ts: Math.floor(Date.now() / 1000),
      last_failure_ts: Math.floor(Date.now() / 1000),
      last_request_id: makeRequestId(),
      last_status: 401,
    })

    runtime.addEvent({
      request_id: makeRequestId(),
      ts: Math.floor(Date.now() / 1000),
      method: 'POST',
      path: `/github-package-update/${params.slug}`,
      status: 401,
      action: 'github-webhook',
      duration_ms: 90,
      meta: { signature: 'invalid' },
    })

    return HttpResponse.json({ status: 'recorded' }, { status: 200 })
  }),
]

export { handlers }
