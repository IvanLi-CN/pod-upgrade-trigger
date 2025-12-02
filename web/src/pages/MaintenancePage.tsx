import { Icon } from '@iconify/react'
import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useApi } from '../hooks/useApi'
import { useToast } from '../components/Toast'

type SettingsResources = {
  resources?: {
    state_dir?: { path?: string }
    database_file?: FileStats
    debug_payload?: FileStats
    web_dist?: FileStats
  }
}

type FileStats = {
  exists: boolean
  is_dir?: boolean
  size?: number
  modified_ts?: number | null
  path?: string
}

export default function MaintenancePage() {
  const { getJson, postJson } = useApi()
  const { pushToast } = useToast()
  const navigate = useNavigate()
  const [resources, setResources] = useState<SettingsResources['resources'] | null>(null)
  const [maxAgeHours, setMaxAgeHours] = useState('24')
  const [prunePending, setPrunePending] = useState(false)

  useEffect(() => {
    let cancelled = false
    ;(async () => {
      try {
        const settings = await getJson<SettingsResources>('/api/settings')
        if (!cancelled) {
          setResources(settings.resources ?? null)
        }
      } catch (err) {
        console.error('Failed to load settings for maintenance', err)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [getJson])

  const triggerPrune = async () => {
    setPrunePending(true)
    type PruneStateResponse = {
      tokens_removed?: number
      locks_removed?: number
      legacy_dirs_removed?: number
      dry_run?: boolean
      max_age_hours?: number
      task_id?: string | null
    }
    try {
      const maxAge = Number(maxAgeHours) || 24
      const result = await postJson<PruneStateResponse>('/api/prune-state', {
        max_age_hours: maxAge,
        dry_run: false,
      })
      pushToast({
        variant: 'success',
        title: '清理完成',
        message: `tokens=${result.tokens_removed ?? 0}, locks=${result.locks_removed ?? 0}`,
      })

      if (result.task_id) {
        pushToast({
          variant: 'info',
          title: '已创建维护任务',
          message: '正在 /tasks 中跟踪清理进度。',
        })
        navigate(`/tasks?task_id=${encodeURIComponent(result.task_id)}`)
      }
    } catch (err) {
      const message =
        err && typeof err === 'object' && 'message' in err && err.message
          ? String(err.message)
          : 'Unknown error'
      pushToast({
        variant: 'error',
        title: '清理失败',
        message,
      })
    } finally {
      setPrunePending(false)
    }
  }

  const downloadDebugPayload = () => {
    window.location.href = '/last_payload.bin'
  }

  const dbStats = resources?.database_file
  const payloadStats = resources?.debug_payload
  const webDistStats = resources?.web_dist

  const describeStats = (stats?: FileStats) => {
    if (!stats) return '未知'
    if (!stats.exists) return '缺失'
    const size = typeof stats.size === 'number' ? `${stats.size} bytes` : 'size ?'
    const ts =
      typeof stats.modified_ts === 'number'
        ? new Date(stats.modified_ts * 1000).toLocaleString()
        : 'time ?'
    return `${size} · ${ts}`
  }

  return (
    <div className="space-y-6">
      <section id="status" className="card bg-base-100 shadow">
        <div className="card-body gap-3">
          <h2 className="text-lg font-semibold uppercase tracking-wide text-base-content/70">
            状态目录检查
          </h2>
          <div className="overflow-x-auto">
            <table className="table table-sm">
              <thead>
                <tr>
                  <th>资源</th>
                  <th>状态</th>
                  <th>备注</th>
                </tr>
              </thead>
              <tbody>
                <tr>
                  <td className="font-mono text-xs">pod-upgrade-trigger.db</td>
                  <td>
                    <span
                      className={`badge badge-xs ${
                        dbStats?.exists ? 'badge-success' : 'badge-error'
                      }`}
                    >
                      {dbStats?.exists ? '存在' : '缺失'}
                    </span>
                  </td>
                  <td className="text-xs">{describeStats(dbStats)}</td>
                </tr>
                <tr>
                  <td className="font-mono text-xs">last_payload.bin</td>
                  <td>
                    <span
                      className={`badge badge-xs ${
                        payloadStats?.exists ? 'badge-success' : 'badge-warning'
                      }`}
                    >
                      {payloadStats?.exists ? '存在' : '缺失'}
                    </span>
                  </td>
                  <td className="text-xs">
                    {describeStats(payloadStats)} · 仅当签名失败时会生成
                  </td>
                </tr>
                <tr>
                  <td className="font-mono text-xs">web/dist</td>
                  <td>
                    <span
                      className={`badge badge-xs ${
                        webDistStats?.exists ? 'badge-success' : 'badge-error'
                      }`}
                    >
                      {webDistStats?.exists ? '存在' : '缺失'}
                    </span>
                  </td>
                  <td className="text-xs">
                    {describeStats(webDistStats)} · 构建前端：npm run build
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
        </div>
      </section>

      <section id="ratelimit" className="card bg-base-100 shadow">
        <div className="card-body gap-3">
          <div className="flex items-center justify-between">
            <h2 className="text-lg font-semibold uppercase tracking-wide text-base-content/70">
              速率限制清理
            </h2>
            <span className="text-[11px] text-base-content/60">
              对应 POST /api/prune-state
            </span>
          </div>
          <div className="flex flex-wrap items-end gap-3">
            <label className="form-control w-40">
              <span className="label-text text-xs">最大保留时间（小时）</span>
              <input
                className="input input-xs input-bordered"
                value={maxAgeHours}
                onChange={(event) => setMaxAgeHours(event.target.value)}
              />
            </label>
            <button
              type="button"
              className="btn btn-primary btn-sm gap-1"
              onClick={triggerPrune}
              disabled={prunePending}
            >
              <Icon icon="mdi:broom" className="text-lg" />
              清理
            </button>
          </div>
          <p className="text-xs text-base-content/60">
            该操作会删除超过指定时间窗口的 rate_limit_tokens 和 image_locks 记录，并清理旧版
            SQLite/锁文件。
          </p>
        </div>
      </section>

      <section className="card bg-base-100 shadow">
        <div className="card-body gap-3">
          <h2 className="text-lg font-semibold uppercase tracking-wide text-base-content/70">
            下载调试包
          </h2>
          <p className="text-xs text-base-content/70">
            last_payload.bin 仅在最近一次 GitHub HMAC 签名失败时生成，用于对齐签名算法与原始 payload。
          </p>
          <button
            type="button"
            className="btn btn-sm btn-outline gap-1"
            onClick={downloadDebugPayload}
          >
            <Icon icon="mdi:download" className="text-lg" />
            下载 last_payload.bin
          </button>
        </div>
      </section>
    </div>
  )
}
