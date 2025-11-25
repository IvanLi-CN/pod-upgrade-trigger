/// <reference lib="dom" />
import { faker } from '@faker-js/faker'
import cloneDeep from 'lodash-es/cloneDeep'

export type MockProfile =
  | 'happy-path'
  | 'empty-state'
  | 'rate-limit-hot'
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
  created_at: number
}

export type ManualService = {
  slug: string
  unit: string
  display_name: string
  default_image?: string | null
  github_path?: string
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
    PODUP_MANUAL_TOKEN_configured?: boolean
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

function buildEvents(now: number, profile: MockProfile): MockEvent[] {
  if (profile === 'empty-state') return []

  const baseTs = now - 3600
  const common: MockEvent[] = [
    {
      id: 1,
      request_id: makeRequestId(),
      ts: baseTs + 120,
      method: 'POST',
      path: '/api/manual/trigger',
      status: 200,
      action: 'manual-trigger',
      duration_ms: 320,
      meta: { caller: 'seed', reason: 'daily maintenance' },
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
      slug: 'svc-alpha',
      unit: 'svc-alpha.service',
      display_name: 'Alpha Deploy',
      default_image: 'ghcr.io/example/svc-alpha:stable',
      github_path: 'example/svc-alpha',
    },
    {
      slug: 'svc-beta',
      unit: 'svc-beta.service',
      display_name: 'Beta Deploy',
      default_image: 'ghcr.io/example/svc-beta:stable',
      github_path: 'example/svc-beta',
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
      PODUP_MANUAL_TOKEN_configured: profile !== 'auth-error',
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

function buildInitialData(profile: MockProfile): RuntimeData {
  const now = Math.floor(Date.now() / 1000)
  return {
    now,
    events: buildEvents(now, profile),
    services: buildServices(profile),
    webhooks: buildWebhooks(now, profile),
    locks: buildLocks(now, profile),
    settings: buildSettings(now, profile),
    config: buildConfig(),
    lastPayload: new Uint8Array([0xde, 0xad, 0xbe, 0xef]),
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

  // biome-ignore lint/correctness/noUnusedPrivateClassMembers: used in constructor
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

  // biome-ignore lint/correctness/noUnusedPrivateClassMembers: used in constructor
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
