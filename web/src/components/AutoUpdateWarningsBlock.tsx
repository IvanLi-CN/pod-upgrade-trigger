import { Icon } from '@iconify/react'
import type { TaskLogEntry } from '../domain/tasks'

type AutoUpdateWarningsBlockProps = {
  summary: TaskLogEntry
  details: TaskLogEntry[]
}

const formatTs = (ts?: number | null) => {
  if (!ts || ts <= 0) return '--'
  return new Date(ts * 1000).toLocaleString()
}

export function AutoUpdateWarningsBlock({
  summary,
  details,
}: AutoUpdateWarningsBlockProps) {
  const countFromDetails = details.length

  let countFromMeta: number | null = null
  if (summary.meta && typeof summary.meta === 'object') {
    const meta = summary.meta as { [key: string]: unknown }
    const warnings = meta.warnings
    if (Array.isArray(warnings)) {
      countFromMeta = warnings.length
    }
  }

  const totalCount =
    typeof countFromMeta === 'number' && countFromMeta > 0
      ? countFromMeta
      : countFromDetails

  const hasErrorDetail = details.some((log) => log.level === 'error')

  const title =
    totalCount > 0 ? `Auto-update warnings (${totalCount})` : 'Auto-update warnings'

  return (
    <div className="space-y-2 rounded border border-warning/70 bg-warning/5 px-2 py-2 text-[11px]">
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-1">
          <Icon
            icon={hasErrorDetail ? 'mdi:alert-octagon-outline' : 'mdi:alert-outline'}
            className={hasErrorDetail ? 'text-error text-sm' : 'text-warning text-sm'}
          />
          <span className="font-semibold">{title}</span>
        </div>
        <span className="text-[10px] text-base-content/60">
          {formatTs(summary.ts)}
        </span>
      </div>
      <p className="text-[11px] text-base-content/80">{summary.summary}</p>
      {details.length > 0 ? (
        <div className="space-y-1">
          {details.map((log) => {
            const isError = log.level === 'error'
            return (
              <div
                key={log.id}
                className={`flex flex-col gap-0.5 rounded border px-2 py-1 ${
                  isError ? 'border-error/70 bg-error/10' : 'border-warning/60 bg-warning/10'
                }`}
              >
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div className="flex items-center gap-2">
                    <span
                      className={`badge badge-xs ${
                        isError ? 'badge-error' : 'badge-warning'
                      }`}
                    >
                      {log.level}
                    </span>
                    {log.unit ? (
                      <span className="badge badge-ghost badge-xs">{log.unit}</span>
                    ) : null}
                  </div>
                  <span className="text-[10px] text-base-content/60">
                    {formatTs(log.ts)}
                  </span>
                </div>
                <p className="text-[11px] text-base-content/80">{log.summary}</p>
              </div>
            )
          })}
        </div>
      ) : null}
    </div>
  )
}

