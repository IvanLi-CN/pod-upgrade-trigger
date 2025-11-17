import { Icon } from '@iconify/react'
import { useEffect, useMemo, useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import { useApi } from '../hooks/useApi'
import { useToast } from '../components/Toast'

type WebhookUnit = {
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

type WebhooksStatusResponse = {
  now: number
  secret_configured: boolean
  units: WebhookUnit[]
}

type LockEntry = {
  bucket: string
  acquired_at: number
  age_secs: number
}

type ImageLocksResponse = {
  now: number
  locks: LockEntry[]
}

export default function WebhooksPage() {
  const { getJson } = useApi()
  const { pushToast } = useToast()
  const [status, setStatus] = useState<WebhooksStatusResponse | null>(null)
  const [locks, setLocks] = useState<LockEntry[]>([])
  const [_searchParams, setSearchParams] = useSearchParams()

  useEffect(() => {
    let cancelled = false
    ;(async () => {
      try {
        const data = await getJson<WebhooksStatusResponse>('/api/webhooks/status')
        if (!cancelled) {
          setStatus(data)
        }
      } catch (err) {
        console.error('Failed to load webhook status', err)
      }
    })()

    ;(async () => {
      try {
        const data = await getJson<ImageLocksResponse>('/api/image-locks')
        if (!cancelled && Array.isArray(data.locks)) {
          setLocks(data.locks)
        }
      } catch (err) {
        console.error('Failed to load image locks', err)
      }
    })()

    return () => {
      cancelled = true
    }
  }, [getJson])

  const handleCopy = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text)
      pushToast({
        variant: 'success',
        title: '已复制',
        message: text,
      })
    } catch {
      pushToast({
        variant: 'warning',
        title: '复制失败',
        message: '浏览器未允许访问剪贴板。',
      })
    }
  }

  const handleReleaseLock = async (bucket: string) => {
    try {
      await fetch(`/api/image-locks/${encodeURIComponent(bucket)}`, {
        method: 'DELETE',
      })
      setLocks((prev) => prev.filter((lock) => lock.bucket !== bucket))
      pushToast({
        variant: 'info',
        title: '锁已释放',
        message: bucket,
      })
    } catch (err) {
      const message =
        err && typeof err === 'object' && 'message' in err && err.message
          ? String(err.message)
          : 'Unknown error'
      pushToast({
        variant: 'error',
        title: '释放失败',
        message,
      })
    }
  }

  const isSecretOk = status?.secret_configured ?? false

  const sortedLocks = useMemo(
    () => [...locks].sort((a, b) => a.acquired_at - b.acquired_at),
    [locks],
  )

  const formatTs = (ts?: number | null) => {
    if (!ts || ts <= 0) return '--'
    return new Date(ts * 1000).toLocaleString()
  }

  const openEventsForUnit = (unit: WebhookUnit) => {
    const path = unit.webhook_url
    setSearchParams({ path_prefix: path })
  }

  return (
    <div className="space-y-6">
      <section className="card bg-base-100 shadow">
        <div className="card-body gap-3">
          <div className="flex items-center justify-between">
            <h2 className="text-sm font-semibold uppercase tracking-wide text-base-content/70">
              GitHub Webhooks
            </h2>
            <span
              className={`badge badge-sm ${
                isSecretOk ? 'badge-success' : 'badge-warning'
              }`}
            >
              secret {isSecretOk ? 'configured' : 'missing'}
            </span>
          </div>
          <div className="space-y-3">
            {status?.units?.length ? (
              status.units.map((unit) => (
                <div
                  key={unit.slug}
                  className="flex flex-col gap-2 rounded-lg border border-base-200 bg-base-100 px-3 py-2 text-xs md:flex-row md:items-center"
                >
                  <div className="flex min-w-0 flex-1 flex-col gap-1">
                    <div className="flex items-center gap-2">
                      <span className="font-semibold">{unit.unit}</span>
                      <span className="badge badge-ghost badge-xs">{unit.slug}</span>
                      <span
                        className={`badge badge-xs ${
                          unit.hmac_ok ? 'badge-success' : 'badge-error'
                        }`}
                      >
                        HMAC {unit.hmac_ok ? 'OK' : 'Error'}
                      </span>
                    </div>
                    <div className="flex flex-wrap items-center gap-1 text-[10px]">
                      <span className="badge badge-outline badge-xs gap-1">
                        <Icon icon="mdi:webhook" />
                        {unit.webhook_url}
                      </span>
                      <span className="badge badge-outline badge-xs gap-1">
                        <Icon icon="mdi:refresh" />
                        {unit.redeploy_url}
                      </span>
                      {unit.expected_image && (
                        <span className="badge badge-outline badge-xs gap-1">
                          <Icon icon="mdi:docker" />
                          {unit.expected_image}
                        </span>
                      )}
                    </div>
                    <div className="mt-1 flex flex-wrap items-center gap-2 text-[10px] text-base-content/70">
                      <span>last · {formatTs(unit.last_ts ?? null)}</span>
                      <span>success · {formatTs(unit.last_success_ts ?? null)}</span>
                      <span>failure · {formatTs(unit.last_failure_ts ?? null)}</span>
                      {!unit.hmac_ok && unit.hmac_last_error && (
                        <span className="text-error">
                          hmac · {unit.hmac_last_error}
                        </span>
                      )}
                    </div>
                  </div>
                  <div className="flex items-center gap-2 md:flex-col md:items-end">
                    <button
                      type="button"
                      className="btn btn-xs btn-outline"
                      onClick={() => handleCopy(unit.webhook_url)}
                    >
                      <Icon icon="mdi:content-copy" className="text-sm" />
                      复制 URL
                    </button>
                    <button
                      type="button"
                      className="btn btn-xs btn-ghost gap-1"
                      onClick={() => openEventsForUnit(unit)}
                    >
                      <Icon icon="mdi:open-in-new" className="text-sm" />
                      查看事件
                    </button>
                  </div>
                </div>
              ))
            ) : (
              <p className="text-xs text-base-content/60">
                未检测到任何 GitHub webhook 单元。请检查 WEBHOOK_MANUAL_UNITS 和 systemd
                配置。
              </p>
            )}
          </div>
        </div>
      </section>

      <section className="card bg-base-100 shadow">
        <div className="card-body gap-3">
          <div className="flex items-center justify-between">
            <h2 className="text-sm font-semibold uppercase tracking-wide text-base-content/70">
              镜像锁
            </h2>
            <span className="text-[11px] text-base-content/60">
              来自 /api/image-locks · 超过窗口后会自动清理
            </span>
          </div>
          {sortedLocks.length === 0 ? (
            <p className="text-xs text-base-content/60">当前没有被锁定的镜像。</p>
          ) : (
            <div className="overflow-x-auto">
              <table className="table table-xs">
                <thead>
                  <tr>
                    <th>Bucket</th>
                    <th>Acquired at</th>
                    <th>Age (s)</th>
                    <th>Actions</th>
                  </tr>
                </thead>
                <tbody>
                  {sortedLocks.map((lock) => {
                    const remaining = Math.max(0, 3600 - lock.age_secs)
                    return (
                      <tr key={lock.bucket}>
                        <td className="font-mono text-[11px]">{lock.bucket}</td>
                        <td>{formatTs(lock.acquired_at)}</td>
                        <td>
                          {lock.age_secs} · unlock in{' '}
                          {Math.round(remaining / 60)} min
                        </td>
                        <td>
                          <button
                            type="button"
                            className="btn btn-xs btn-outline"
                            onClick={() => handleReleaseLock(lock.bucket)}
                          >
                            释放
                          </button>
                        </td>
                      </tr>
                    )
                  })}
                </tbody>
              </table>
            </div>
          )}
          <div className="alert alert-info mt-2 text-xs">
            <Icon icon="mdi:information-outline" className="text-lg" />
            <span>
              镜像锁用于保护 GitHub Container Registry 的请求速率。只有在确认无并发风险时才手动释放。
            </span>
          </div>
        </div>
      </section>
    </div>
  )
}
