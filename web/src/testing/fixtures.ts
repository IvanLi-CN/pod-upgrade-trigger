import type { TaskLogEntry } from '../domain/tasks'

const nowInSeconds = () => Math.floor(Date.now() / 1000)

export type AutoUpdateWarningsOptions = {
  baseTs?: number
  includeError?: boolean
}

export function makeAutoUpdateWarningsSummary(
  opts: AutoUpdateWarningsOptions = {},
): TaskLogEntry {
  const baseTs = opts.baseTs ?? nowInSeconds()
  const includeError = opts.includeError ?? true
  const warningCount = includeError ? 2 : 1

  return {
    id: 1,
    ts: baseTs,
    level: 'warning',
    action: 'auto-update-warnings',
    status: includeError ? 'unknown' : 'failed',
    summary: includeError
      ? 'Auto-update completed with warnings and 1 error'
      : 'Last auto-update completed with warnings',
    meta: {
      unit: 'podman-auto-update.service',
      warnings: [
        { type: 'dry-run-error', at: '2024-01-01T00:00:00Z' },
        ...(includeError ? [{ type: 'auto-update-error', at: '2024-01-01T00:01:00Z' }] : []),
      ],
      warning_count: warningCount,
    },
  }
}

export function makeAutoUpdateWarningDetails(
  opts: AutoUpdateWarningsOptions = {},
): TaskLogEntry[] {
  const baseTs = opts.baseTs ?? nowInSeconds()
  const includeError = opts.includeError ?? true

  const details: TaskLogEntry[] = [
    {
      id: 2,
      ts: baseTs + 15,
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
  ]

  if (includeError) {
    details.push({
      id: 3,
      ts: baseTs + 25,
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
    })
  }

  return details
}

export function makeAutoUpdateWarningsProps(
  opts: AutoUpdateWarningsOptions = {},
): { summary: TaskLogEntry; details: TaskLogEntry[] } {
  const baseTs = opts.baseTs ?? nowInSeconds()
  const includeError = opts.includeError ?? true
  const summary = makeAutoUpdateWarningsSummary({ baseTs, includeError })
  const details = makeAutoUpdateWarningDetails({ baseTs, includeError })
  return { summary, details }
}
