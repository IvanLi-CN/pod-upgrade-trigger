import { http, HttpResponse } from 'msw'
import { runtime } from './runtime'

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
    return HttpResponse.json(runtime.cloneData().settings, { headers: JSON_HEADERS })
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

    const triggered = services.map((svc) => ({
      unit: svc.unit,
      status: body.dry_run ? 'dry-run' : 'triggered',
      message: 'ok',
    }))

    const requestId = runtime.addEvent({
      request_id: body.caller?.toString() ?? `manual-${Date.now()}`,
      ts: Math.floor(Date.now() / 1000),
      method: 'POST',
      path: '/api/manual/trigger',
      status: 200,
      action: 'manual-trigger',
      duration_ms: 180,
      meta: body,
    }).request_id

    return HttpResponse.json({
      triggered,
      dry_run: Boolean(body.dry_run),
      request_id: requestId,
      caller: body.caller ?? null,
      reason: body.reason ?? null,
    }, { headers: JSON_HEADERS })
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

    const status = body.dry_run ? 'dry-run' : 'triggered'

    runtime.addEvent({
      request_id: makeRequestId(),
      ts: Math.floor(Date.now() / 1000),
      method: 'POST',
      path: `/api/manual/services/${service.slug}`,
      status: body.dry_run ? 202 : 200,
      action: 'manual-trigger',
      duration_ms: 140,
      meta: { service: service.slug, ...body },
    })

    return HttpResponse.json({
      unit: service.unit,
      status,
      request_id: makeRequestId(),
    }, { headers: JSON_HEADERS })
  }),

  http.get('/api/webhooks/status', async ({ request }) => {
    const url = new URL(request.url)
    const failure = degradedGuard(url) || authGuard(url) || maybeFailure()
    if (failure) return failure
    await withLatency()

    const data = runtime.cloneData().webhooks
    return HttpResponse.json({ ...data, now: Math.floor(Date.now() / 1000) }, { headers: JSON_HEADERS })
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

    runtime.updateLocks([])

    return HttpResponse.json({
      tokens_removed: 4,
      locks_removed: 3,
      legacy_dirs_removed: 1,
      dry_run: Boolean(body.dry_run),
      max_age_hours: maxAge,
    }, { headers: JSON_HEADERS })
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
