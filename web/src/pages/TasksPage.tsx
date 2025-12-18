import { Icon } from '@iconify/react'
import { useEffect, useMemo, useState } from 'react'
import { Link, useSearchParams } from 'react-router-dom'
import type {
  CommandMeta,
  Task,
  TaskStatus,
  TaskDetailResponse,
  TasksListResponse,
  TaskLogEntry,
  TaskLogLevel,
} from '../domain/tasks'
import { isCommandMeta } from '../domain/tasks'
import { useApi } from '../hooks/useApi'
import { usePresence } from '../hooks/usePresence'
import { usePrefersReducedMotion } from '../hooks/usePrefersReducedMotion'
import { useToast } from '../components/Toast'
import { AutoUpdateWarningsBlock } from '../components/AutoUpdateWarningsBlock'
import { TaskLogMetaDetails } from '../components/TaskLogMetaDetails'

type TaskCategory = 'all' | 'manual' | 'webhook' | 'automatic' | 'maintenance'

type StatusFilter = TaskStatus | ''

const PAGE_SIZE = 20
const POLL_INTERVAL_MS = 7000
const DETAIL_POLL_INTERVAL_MS = 3000

function buildMockCommandMetaFallback(
  log: TaskLogEntry,
  mockEnabled: boolean,
): CommandMeta | null {
  if (!mockEnabled) return null
  if (log.action !== 'image-pull' && log.action !== 'restart-unit') return null
  if (!log.meta || typeof log.meta !== 'object') return null

  const candidate = log.meta as { [key: string]: unknown }
  const units =
    Array.isArray(candidate.units) && candidate.units.every((u) => typeof u === 'string')
      ? (candidate.units as string[])
      : null
  if (!units || units.length === 0) return null

  const firstUnit = units[0]
  if (typeof firstUnit !== 'string' || !firstUnit.endsWith('.service')) return null
  const mainService = firstUnit.slice(0, -'.service'.length)

  if (log.action === 'image-pull') {
    return {
      type: 'command',
      command: `podman pull ghcr.io/example/${mainService}:main`,
      argv: ['podman', 'pull', `ghcr.io/example/${mainService}:main`],
      stdout: 'pulling from registry.example...\ncomplete',
      stderr: 'warning: using cached image layer metadata',
      exit: 'exit=0',
      units,
    }
  }

  // restart-unit fallback
  return {
    type: 'command',
    command: `systemctl --user restart ${firstUnit}`,
    argv: ['systemctl', '--user', 'restart', firstUnit],
    stdout: `restarted ${firstUnit}\nreloaded dependencies`,
    stderr: '',
    exit: 'exit=0',
    ok: units,
  }
}

export default function TasksPage() {
  const { status: appStatus, getJson, postJson, mockEnabled } = useApi()
  const { pushToast } = useToast()
  const [params, setParams] = useSearchParams()
  const [tasks, setTasks] = useState<Task[]>([])
  const [total, setTotal] = useState(0)
  const [page, setPage] = useState(1)
  const [hasNext, setHasNext] = useState(false)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const [category, setCategory] = useState<TaskCategory>('all')
  const [statusFilter, setStatusFilter] = useState<StatusFilter>('')
  const [kindFilter, setKindFilter] = useState<string>('')
  const [unitQuery, setUnitQuery] = useState('')

  const [drawerTask, setDrawerTask] = useState<TaskDetailResponse | null>(null)
  const [drawerLogs, setDrawerLogs] = useState<TaskLogEntry[] | null>(null)
  const [drawerLoading, setDrawerLoading] = useState(false)
  const [drawerError, setDrawerError] = useState<string | null>(null)
  const [expandedCommandLogs, setExpandedCommandLogs] = useState<Record<number, boolean>>({})
  const urlTaskId = (() => {
    const raw = params.get('task_id')
    const trimmed = raw?.trim()
    return trimmed ? trimmed : null
  })()
  const prefersReducedMotion = usePrefersReducedMotion()
  const drawerPresence = usePresence(Boolean(urlTaskId), {
    enterMs: prefersReducedMotion ? 0 : 200,
    exitMs: prefersReducedMotion ? 0 : 200,
  })
  const [drawerTaskIdSnapshot, setDrawerTaskIdSnapshot] = useState<string | null>(urlTaskId)
  const drawerEffectiveTaskId = urlTaskId ?? drawerTaskIdSnapshot

  const effectiveKindFilter = useMemo(() => {
    if (kindFilter) return kindFilter
    switch (category) {
      case 'manual':
        return 'manual'
      case 'webhook':
        return 'github-webhook'
      case 'automatic':
        return 'scheduler'
      case 'maintenance':
        return 'maintenance'
      default:
        return ''
    }
  }, [category, kindFilter])

  useEffect(() => {
    let cancelled = false

    const load = async () => {
      setLoading(true)
      setError(null)
      try {
        const query = new URLSearchParams()
        query.set('page', String(page))
        query.set('per_page', String(PAGE_SIZE))
        if (statusFilter) query.set('status', statusFilter)
        if (effectiveKindFilter) query.set('kind', effectiveKindFilter)
        if (unitQuery.trim()) query.set('unit', unitQuery.trim())

        const data = await getJson<TasksListResponse>(`/api/tasks?${query.toString()}`)
        if (cancelled) return
        setTasks(Array.isArray(data.tasks) ? data.tasks : [])
        setTotal(data.total ?? 0)
        setHasNext(Boolean(data.has_next))
      } catch (err) {
        if (cancelled) return
        const message =
          err && typeof err === 'object' && 'message' in err && err.message
            ? String(err.message)
            : '加载任务列表失败'
        setError(message)
      } finally {
        if (!cancelled) {
          setLoading(false)
        }
      }
    }

    load()

    const interval = window.setInterval(load, POLL_INTERVAL_MS)

    return () => {
      cancelled = true
      window.clearInterval(interval)
    }
  }, [effectiveKindFilter, getJson, page, statusFilter, unitQuery])

  const handleChangeCategory = (next: TaskCategory) => {
    setCategory(next)
    setPage(1)
    if (next === 'all') {
      setKindFilter('')
    }
  }

  const handleChangeStatus = (value: string) => {
    setStatusFilter(value as StatusFilter)
    setPage(1)
  }

  const handleChangeKind = (value: string) => {
    setKindFilter(value)
    setPage(1)
  }

  const handleChangeUnitQuery = (value: string) => {
    setUnitQuery(value)
    setPage(1)
  }

  useEffect(() => {
    if (urlTaskId) {
      setDrawerTaskIdSnapshot(urlTaskId)
    }
  }, [urlTaskId])

  const handleRowClick = (task: Task) => {
    const next = new URLSearchParams(params)
    next.set('task_id', task.task_id)
    setParams(next, { replace: true })
  }

  const handleCloseDrawer = () => {
    const next = new URLSearchParams(params)
    next.delete('task_id')
    setParams(next, { replace: true })
  }

  useEffect(() => {
    if (!urlTaskId) return

    let cancelled = false
    let timeoutId: number | undefined

    const loadDetail = async () => {
      if (cancelled) return
      setDrawerLoading(true)
      setDrawerError(null)
      try {
        const data = await getJson<TaskDetailResponse>(`/api/tasks/${encodeURIComponent(urlTaskId)}`)
        if (cancelled) return
        setDrawerTask(data)
        setDrawerLogs(Array.isArray(data.logs) ? data.logs : [])
        // 将详情中的最新状态同步到列表
        setTasks((prev) =>
          prev.map((task) =>
            task.task_id === data.task_id ? { ...task, ...data } : task,
          ),
        )
        if (data.status === 'running') {
          timeoutId = window.setTimeout(loadDetail, DETAIL_POLL_INTERVAL_MS)
        }
      } catch (err) {
        if (cancelled) return
        const message =
          err && typeof err === 'object' && 'message' in err && err.message
            ? String(err.message)
            : '加载任务详情失败'
        setDrawerError(message)
      } finally {
        if (!cancelled) {
          setDrawerLoading(false)
        }
      }
    }

    loadDetail()

    return () => {
      cancelled = true
      if (timeoutId !== undefined) {
        window.clearTimeout(timeoutId)
      }
    }
  }, [getJson, urlTaskId])

  useEffect(() => {
    if (!urlTaskId) return
    setExpandedCommandLogs({})
  }, [urlTaskId])

  // 当首次看到命令型日志时，默认展开命令输出（避免任务执行太快导致永远折叠）。
  useEffect(() => {
    if (!drawerLogs || drawerLogs.length === 0) return
    setExpandedCommandLogs((prev) => {
      let changed = false
      const next = { ...prev }
      for (const log of drawerLogs) {
        if (next[log.id] !== undefined) continue
        if (!isCommandMeta(log.meta) && !buildMockCommandMetaFallback(log, mockEnabled)) continue
        next[log.id] = true
        changed = true
      }
      return changed ? next : prev
    })
  }, [drawerLogs, mockEnabled])

  const drawerStatus = drawerTask?.status

  useEffect(() => {
    if (!urlTaskId) return
    if (drawerStatus !== 'running') return
    if (typeof EventSource === 'undefined') return

    let cancelled = false
    const url = `/sse/task-logs?task_id=${encodeURIComponent(urlTaskId)}`
    let source: EventSource
    try {
      source = new EventSource(url)
    } catch {
      // 创建失败时静默降级为仅依赖 HTTP 轮询。
      return
    }

    const handleLog = (event: MessageEvent) => {
      if (cancelled) return
      try {
        const payload = JSON.parse(event.data) as TaskLogEntry
        setDrawerLogs((prev) => {
          if (!prev || prev.length === 0) return [payload]
          const index = prev.findIndex((entry) => entry.id === payload.id)
          if (index === -1) return [...prev, payload]
          const next = prev.slice()
          next[index] = payload
          return next
        })
      } catch {
        // ignore malformed mock payload
      }
    }

    const handleEnd = () => {
      if (!cancelled) {
        source.close()
      }
    }

    source.addEventListener('log', handleLog)
    source.addEventListener('end', handleEnd)
    source.onerror = () => {
      if (!cancelled) {
        source.close()
      }
    }

    return () => {
      cancelled = true
      source.removeEventListener('log', handleLog)
      source.removeEventListener('end', handleEnd)
      source.close()
    }
  }, [drawerStatus, urlTaskId])

  useEffect(() => {
    if (drawerPresence.present) return
    setDrawerTaskIdSnapshot(null)
    setDrawerTask(null)
    setDrawerLogs(null)
    setDrawerError(null)
    setDrawerLoading(false)
    setExpandedCommandLogs({})
  }, [drawerPresence.present])

  const pageLabel = useMemo(() => {
    if (!total || total <= PAGE_SIZE) return `第 ${page} 页`
    const maxPage = Math.ceil(total / PAGE_SIZE)
    return `第 ${page} / ${maxPage} 页`
  }, [page, total])
  const formatTs = (ts?: number | null) => {
    if (!ts || ts <= 0) return '--'
    return new Date(ts * 1000).toLocaleString()
  }

  const formatTimeWithMs = (ts?: number | null) => {
    if (!ts || ts <= 0) return '--'
    const d = new Date(ts * 1000)
    const pad = (value: number, width = 2) => String(value).padStart(width, '0')
    const hh = pad(d.getHours())
    const mm = pad(d.getMinutes())
    const ss = pad(d.getSeconds())
    const ms = pad(d.getMilliseconds(), 3)
    return `${hh}:${mm}:${ss}.${ms}`
  }

  const formatDuration = (task: Task) => {
    const start = task.started_at ?? task.created_at
    if (!start || start <= 0) return '--'
    const end =
      task.finished_at && task.finished_at > 0
        ? task.finished_at
        : Math.floor(appStatus.now.getTime() / 1000)
    const delta = Math.max(0, end - start)
    if (delta < 60) return `${delta}s`
    if (delta < 3600) return `${Math.floor(delta / 60)}m`
    return `${Math.floor(delta / 3600)}h`
  }

  const formatKindLabel = (kind: Task['kind']) => {
    switch (kind) {
      case 'manual':
        return 'Manual'
      case 'github-webhook':
        return 'Webhook'
      case 'scheduler':
        return 'Scheduler'
      case 'maintenance':
        return 'Maintenance'
      case 'internal':
        return 'Internal'
      case 'other':
        return 'Other'
    }
  }

  const statusBadgeClass = (status: TaskStatus, level?: TaskLogLevel) => {
    // For log timeline entries we prefer the log level over the raw status so
    // that warning/error entries are immediately visible even when the task as
    // a whole succeeded.
    if (level === 'warning') return 'badge-warning'
    if (level === 'error') return 'badge-error'
    switch (status) {
      case 'running':
        return 'badge-info'
      case 'succeeded':
        return 'badge-success'
      case 'failed':
        return 'badge-error'
      case 'cancelled':
        return 'badge-neutral'
      case 'skipped':
        return 'badge-ghost'
      case 'unknown':
        // Unknown is terminal but ambiguous; keep it visually distinct from
        // success by using a warning/amber style.
        return 'badge-warning'
      default:
        return 'badge-warning'
    }
  }

  const renderTaskStatusLabel = (status: TaskStatus) => {
    if (status === 'unknown') return 'Unknown'
    return status
  }

  const logStatusLabel = (log: TaskLogEntry) => {
    if (log.level === 'warning' || log.level === 'error') {
      return log.level
    }
    return log.status
  }

  const unitSummaryText = (task: Task) => {
    const { unit_counts: counts } = task
    if (!counts || !counts.total_units) return '0 units'
    const parts: string[] = []
    if (counts.succeeded) parts.push(`${counts.succeeded} ok`)
    if (counts.failed) parts.push(`${counts.failed} failed`)
    if (counts.cancelled) parts.push(`${counts.cancelled} cancelled`)
    if (counts.running) parts.push(`${counts.running} running`)
    if (parts.length === 0) return `${counts.total_units} units`
    return `${counts.total_units} units · ${parts.join(', ')}`
  }

  const handleCopyCommand = async (command?: string) => {
    if (!command) return
    try {
      await navigator.clipboard.writeText(command)
      pushToast({
        variant: 'success',
        title: '已复制',
        message: command,
      })
    } catch {
      pushToast({
        variant: 'warning',
        title: '复制失败',
        message: '浏览器未允许访问剪贴板。',
      })
    }
  }

  const handleStop = async () => {
    if (!drawerTask) return
    const id = drawerTask.task_id
    try {
      const data = await postJson<TaskDetailResponse>(`/api/tasks/${encodeURIComponent(id)}/stop`, {})
      setDrawerTask(data)
      setTasks((prev) =>
        prev.map((task) => (task.task_id === data.task_id ? { ...task, ...data } : task)),
      )
      pushToast({
        variant: 'info',
        title: '任务已请求停止',
        message: id,
      })
    } catch (err) {
      const message =
        err && typeof err === 'object' && 'message' in err && err.message
          ? String(err.message)
          : '停止任务失败'
      pushToast({
        variant: 'error',
        title: '停止任务失败',
        message,
      })
    }
  }

  const handleForceStop = async () => {
    if (!drawerTask) return
    const id = drawerTask.task_id
    try {
      const data = await postJson<TaskDetailResponse>(
        `/api/tasks/${encodeURIComponent(id)}/force-stop`,
        {},
      )
      setDrawerTask(data)
      setTasks((prev) =>
        prev.map((task) => (task.task_id === data.task_id ? { ...task, ...data } : task)),
      )
      pushToast({
        variant: 'warning',
        title: '已强制停止任务',
        message: id,
      })
    } catch (err) {
      const message =
        err && typeof err === 'object' && 'message' in err && err.message
          ? String(err.message)
          : '强制停止任务失败'
      pushToast({
        variant: 'error',
        title: '强制停止任务失败',
        message,
      })
    }
  }

  const handleRetry = async () => {
    if (!drawerTask) return
    const id = drawerTask.task_id
    try {
      const data = await postJson<TaskDetailResponse>(
        `/api/tasks/${encodeURIComponent(id)}/retry`,
        {},
      )
      setDrawerTask(data)
      setTasks((prev) => [data, ...prev])
      const next = new URLSearchParams(params)
      next.set('task_id', data.task_id)
      setParams(next, { replace: true })
      pushToast({
        variant: 'success',
        title: '已创建重试任务',
        message: data.task_id,
      })
    } catch (err) {
      const message =
        err && typeof err === 'object' && 'message' in err && err.message
          ? String(err.message)
          : '重试任务失败'
      pushToast({
        variant: 'error',
        title: '重试任务失败',
        message,
      })
    }
  }

  const handleExport = () => {
    if (!drawerTask) return
    try {
      const payload = drawerTask
      const blob = new Blob([JSON.stringify(payload, null, 2)], {
        type: 'application/json;charset=utf-8;',
      })
      const url = URL.createObjectURL(blob)
      const link = document.createElement('a')
      link.href = url
      link.setAttribute('download', `task-${payload.task_id}.json`)
      document.body.appendChild(link)
      link.click()
      document.body.removeChild(link)
      URL.revokeObjectURL(url)
    } catch (err) {
      const message =
        err && typeof err === 'object' && 'message' in err && err.message
          ? String(err.message)
          : '导出任务日志失败'
      pushToast({
        variant: 'error',
        title: '导出任务日志失败',
        message,
      })
    }
  }

  return (
    <div className="space-y-6">
      <section className="card bg-base-100 shadow">
        <div className="card-body gap-4">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="space-y-1">
              <h1 className="text-lg font-semibold">任务中心</h1>
              <p className="text-xs text-base-content/70">
                统一查看手动触发、Webhook、调度器与维护任务的执行状态。
              </p>
            </div>
            <div className="flex flex-wrap items-center gap-2 text-[11px] text-base-content/60">
              <span className="badge badge-outline badge-xs">
                <Icon icon="mdi:clock-outline" className="mr-1 text-sm" />
                {appStatus.now.toLocaleTimeString()}
              </span>
              <span className="badge badge-outline badge-xs">
                <Icon icon="mdi:refresh" className="mr-1 text-sm" />
                每 {Math.round(POLL_INTERVAL_MS / 1000)}s 刷新
              </span>
            </div>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            <div className="join join-horizontal">
              {(
                [
                  ['all', '全部'],
                  ['manual', '手动'],
                  ['webhook', 'Webhook'],
                  ['automatic', '自动'],
                  ['maintenance', '维护'],
                ] as [TaskCategory, string][]
              ).map(([value, label]) => (
                <button
                  key={value}
                  type="button"
                  className={`btn btn-xs join-item ${
                    category === value ? 'btn-primary' : 'btn-ghost'
                  }`}
                  onClick={() => handleChangeCategory(value)}
                >
                  {label}
                </button>
              ))}
            </div>
          </div>

          {/* 任务筛选：桌面端一行，label 与控件同一行；移动端整组换行但 label 不换行 */}
          <div className="flex flex-wrap items-center gap-3 text-xs">
            <label className="flex items-center gap-2">
              <span className="whitespace-nowrap text-xs text-base-content/70">
                状态
              </span>
              <select
                className="select select-xs select-bordered w-28"
                value={statusFilter}
                onChange={(event) => handleChangeStatus(event.target.value)}
              >
                <option value="">全部</option>
                <option value="running">running</option>
                <option value="pending">pending</option>
                <option value="succeeded">succeeded</option>
                <option value="failed">failed</option>
                <option value="cancelled">cancelled</option>
                <option value="skipped">skipped</option>
                <option value="unknown">unknown</option>
              </select>
            </label>

            <label className="flex items-center gap-2">
              <span className="whitespace-nowrap text-xs text-base-content/70">
                类型
              </span>
              <select
                className="select select-xs select-bordered w-36"
                value={kindFilter}
                onChange={(event) => handleChangeKind(event.target.value)}
              >
                <option value="">跟随分类</option>
                <option value="manual">manual</option>
                <option value="github-webhook">github-webhook</option>
                <option value="scheduler">scheduler</option>
                <option value="maintenance">maintenance</option>
                <option value="internal">internal</option>
                <option value="other">other</option>
              </select>
            </label>

            <div className="flex min-w-[18rem] flex-1 items-center gap-2">
              <span className="whitespace-nowrap text-xs text-base-content/70">
                Unit / 服务搜索
              </span>
              <input
                className="input input-xs input-bordered flex-1 min-w-0"
                placeholder="按 unit / slug / 名称搜索"
                value={unitQuery}
                onChange={(event) => handleChangeUnitQuery(event.target.value)}
              />
            </div>
          </div>
        </div>
      </section>

      <section className="card bg-base-100 shadow">
        <div className="card-body gap-3">
          <div className="flex items-center justify-between gap-3">
            <div className="space-y-1">
              <h2 className="text-sm font-semibold uppercase tracking-wide text-base-content/70">
                任务列表
              </h2>
              <p className="text-[11px] text-base-content/60">
                数据来自 /api/tasks · 每页 {PAGE_SIZE} 条
              </p>
            </div>
            <div className="flex items-center gap-2 text-[11px]">
              <span className="badge badge-ghost badge-xs">{pageLabel}</span>
              <span className="badge badge-ghost badge-xs">
                共 {total ?? 0} 条
              </span>
            </div>
          </div>

          {error ? (
            <div className="alert alert-error my-2 py-2 text-xs">
              <Icon icon="mdi:alert-circle-outline" className="text-lg" />
              <span>{error}</span>
            </div>
          ) : null}

          <div className="overflow-x-auto">
            <table className="table table-xs">
              <thead>
                <tr>
                  <th>类型</th>
                  <th>状态</th>
                  <th>Units</th>
                  <th>触发来源</th>
                  <th>开始时间</th>
                  <th>耗时</th>
                  <th>摘要</th>
                </tr>
              </thead>
              <tbody>
                {loading && tasks.length === 0 ? (
                  <tr>
                    <td colSpan={7} className="py-6 text-center text-xs text-base-content/60">
                      正在加载任务…
                    </td>
                  </tr>
                ) : null}
                {!loading && tasks.length === 0 && !error ? (
                  <tr>
                    <td colSpan={7} className="py-6 text-center text-xs text-base-content/60">
                      当前没有符合条件的任务记录。
                    </td>
                  </tr>
                ) : null}
                {tasks.map((task) => (
                  <tr
                    key={task.task_id}
                    className="hover:bg-base-200 cursor-pointer"
                    onClick={() => handleRowClick(task)}
                  >
                    <td>
                      <span className="badge badge-ghost badge-xs">
                        {formatKindLabel(task.kind)}
                      </span>
                    </td>
                    <td>
                      <div className="flex items-center gap-1">
                        <span
                          className={`badge badge-xs ${statusBadgeClass(task.status)}`}
                        >
                          {renderTaskStatusLabel(task.status)}
                        </span>
                        {task.has_warnings ? (
                          <span className="badge badge-warning badge-xs gap-1">
                            <Icon icon="mdi:alert-outline" className="text-[11px]" />
                            <span>{task.warning_count ?? 0}</span>
                          </span>
                        ) : null}
                      </div>
                    </td>
                    <td className="max-w-xs truncate text-[11px]">
                      {unitSummaryText(task)}
                    </td>
                    <td className="max-w-xs truncate text-[11px]">
                      {task.trigger.caller
                        ? `${task.trigger.source} · ${task.trigger.caller}`
                        : task.trigger.source}
                    </td>
                    <td>{formatTs(task.started_at ?? task.created_at)}</td>
                    <td>{formatDuration(task)}</td>
                    <td className="max-w-sm truncate text-[11px]">
                      {task.summary ?? '-'}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          <div className="flex items-center justify-between gap-3">
            <div className="join">
              <button
                type="button"
                className="btn btn-xs join-item"
                disabled={page <= 1 || loading}
                onClick={() => setPage((p) => Math.max(1, p - 1))}
              >
                上一页
              </button>
              <button
                type="button"
                className="btn btn-xs join-item"
                disabled={!hasNext || loading}
                onClick={() => setPage((p) => p + 1)}
              >
                下一页
              </button>
            </div>
            <span className="text-[11px] text-base-content/60">
              行点击行为将在任务详情/抽屉实现时补充。
            </span>
          </div>
        </div>
      </section>

      {drawerPresence.present && drawerEffectiveTaskId ? (
        <div
          className={[
            'fixed inset-0 z-40',
            drawerPresence.phase !== 'open' ? 'pointer-events-none' : '',
          ].join(' ')}
        >
          <div
            className={[
              'absolute inset-0 bg-base-300/40 transition-opacity duration-200 motion-reduce:transition-none',
              drawerPresence.visible ? 'opacity-100' : 'opacity-0',
            ].join(' ')}
          />
          <div
            className={[
              'relative ml-auto flex h-full w-full max-w-xl flex-col border-l border-base-300 bg-base-100 shadow-xl',
              'transform transition-transform duration-200 motion-reduce:transition-none',
              drawerPresence.visible ? 'translate-x-0' : 'translate-x-full',
              drawerPresence.phase === 'exiting' ? 'ease-in' : 'ease-out',
            ].join(' ')}
          >
            <div className="flex items-center justify-between border-b border-base-200 px-4 py-3">
              <div className="flex items-center gap-2">
                <span className="text-sm font-semibold">
                  任务详情
                </span>
                {drawerTask ? (
                  <span className="badge badge-ghost badge-xs font-mono">
                    {drawerTask.task_id}
                  </span>
                ) : null}
              </div>
              <button
                type="button"
                className="btn btn-ghost btn-xs"
                onClick={handleCloseDrawer}
              >
                <Icon icon="mdi:close" className="text-lg" />
              </button>
            </div>

            <div className="flex-1 overflow-y-auto px-4 py-3 space-y-3">
              {drawerError ? (
                <div className="alert alert-error py-2 text-xs">
                  <Icon icon="mdi:alert-circle-outline" className="text-lg" />
                  <span>{drawerError}</span>
                </div>
              ) : null}

              {drawerLoading && !drawerTask ? (
                <p className="text-xs text-base-content/60">正在加载任务详情…</p>
              ) : null}

              {drawerTask ? (
                <>
                  <section className="space-y-2">
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <div className="flex items-center gap-2">
                        <span className="badge badge-ghost badge-xs">
                          {formatKindLabel(drawerTask.kind)}
                        </span>
                        <span
                          className={`badge badge-xs ${statusBadgeClass(drawerTask.status)}`}
                        >
                          {renderTaskStatusLabel(drawerTask.status)}
                        </span>
                        {drawerTask.status === 'unknown' ? (
                          <span className="text-[10px] text-warning">
                            Status unknown
                          </span>
                        ) : null}
                        {drawerTask.has_warnings ? (
                          <span className="badge badge-warning badge-xs gap-1">
                            <Icon icon="mdi:alert-outline" className="text-[11px]" />
                            <span>{drawerTask.warning_count ?? 0}</span>
                          </span>
                        ) : null}
                      </div>
                      <div className="flex flex-wrap items-center gap-2 text-[11px] text-base-content/60">
                        <span>
                          创建 · {formatTs(drawerTask.created_at)}
                        </span>
                        <span>
                          起止 · {formatTs(drawerTask.started_at ?? drawerTask.created_at)} →{' '}
                          {formatTs(drawerTask.finished_at ?? null)}
                        </span>
                        <span>耗时 · {formatDuration(drawerTask)}</span>
                      </div>
                    </div>
                    <p className="text-xs text-base-content/70">
                      {drawerTask.summary ?? '暂无摘要说明。'}
                    </p>
                    <div className="flex flex-wrap items-center gap-2 text-[11px] text-base-content/60">
                      <span>来源 · {drawerTask.trigger.source}</span>
                      {drawerTask.trigger.caller ? (
                        <span>caller · {drawerTask.trigger.caller}</span>
                      ) : null}
                      {drawerTask.trigger.reason ? (
                        <span>reason · {drawerTask.trigger.reason}</span>
                      ) : null}
                      {drawerTask.trigger.path ? (
                        <span className="max-w-xs truncate">
                          path · {drawerTask.trigger.path}
                        </span>
                      ) : null}
                    </div>
                  </section>

                  <section className="space-y-2">
                    <h3 className="text-xs font-semibold uppercase tracking-wide text-base-content/70">
                      单元状态
                    </h3>
                    {drawerTask.units.length === 0 ? (
                      <p className="text-[11px] text-base-content/60">该任务未关联任何 unit。</p>
                    ) : (
                      <div className="space-y-1">
                        {drawerTask.units.map((unit) => (
                          <div
                            key={unit.unit}
                            className="flex flex-col gap-1 rounded border border-base-200 bg-base-100 px-2 py-1 text-[11px]"
                          >
                            <div className="flex flex-wrap items-center justify-between gap-2">
                              <div className="flex flex-wrap items-center gap-2">
                                <span className="font-mono text-[11px]">
                                  {unit.unit}
                                </span>
                                {unit.slug ? (
                                  <span className="badge badge-ghost badge-xs">
                                    {unit.slug}
                                  </span>
                                ) : null}
                                <span
                                  className={`badge badge-xs ${statusBadgeClass(unit.status)}`}
                                >
                                  {renderTaskStatusLabel(unit.status)}
                                </span>
                                {unit.phase ? (
                                  <span className="badge badge-outline badge-xs">
                                    {unit.phase}
                                  </span>
                                ) : null}
                              </div>
                              <span className="text-[10px] text-base-content/60">
                                {formatTs(unit.started_at ?? null)} →{' '}
                                {formatTs(unit.finished_at ?? null)}
                              </span>
                            </div>
                            {unit.message ? (
                              <p className="text-[11px] text-base-content/80">
                                {unit.message}
                              </p>
                            ) : null}
                            {unit.error ? (
                              <p className="text-[11px] text-error">
                                error: {unit.error}
                              </p>
                            ) : null}
                          </div>
                        ))}
                      </div>
                    )}
                  </section>

                  <section className="space-y-2">
                    <h3 className="text-xs font-semibold uppercase tracking-wide text-base-content/70">
                      日志时间线
                    </h3>
                    {(() => {
                      const logs = drawerLogs ?? drawerTask.logs
                      if (logs.length === 0) {
                        return (
                          <p className="text-[11px] text-base-content/60">
                            暂无可用日志。
                          </p>
                        )
                      }

                      const autoUpdateSummary = logs.find(
                        (log) => log.action === 'auto-update-warnings',
                      )
                      const autoUpdateDetails = logs.filter(
                        (log) => log.action === 'auto-update-warning',
                      )
                      const timelineLogs =
                        autoUpdateSummary && autoUpdateDetails.length > 0
                          ? logs.filter(
                              (log) =>
                                log.action !== 'auto-update-warnings' &&
                                log.action !== 'auto-update-warning',
                            )
                          : logs

                      return (
                        <div className="space-y-2">
                          {autoUpdateSummary ? (
                            <AutoUpdateWarningsBlock
                              summary={autoUpdateSummary}
                              details={autoUpdateDetails}
                            />
                          ) : null}
                          <div className="space-y-1">
                            {timelineLogs.map((log) => {
                              const isTaskDispatchFailed =
                                log.action === 'task-dispatch-failed'
                              const isImagePrune = log.action === 'image-prune'
                              const isAutoUpdateRun = log.action === 'auto-update-run'
                              const isAutoUpdateRunUnknown =
                                isAutoUpdateRun && log.status === 'unknown'
                              const hasNoSummaryHint =
                                isAutoUpdateRun &&
                                typeof log.summary === 'string' &&
                                log.summary.includes('no JSONL summary found')

                              const commandMeta = isCommandMeta(log.meta)
                                ? (log.meta as CommandMeta)
                                : buildMockCommandMetaFallback(log, mockEnabled)

                              const combinedLines =
                                commandMeta && (commandMeta.stdout || commandMeta.stderr)
                                  ? [
                                      ...(commandMeta.stdout
                                        ? commandMeta.stdout.split('\n').map((text) => ({
                                            stream: 'stdout' as const,
                                            text,
                                          }))
                                        : []),
                                      ...(commandMeta.stderr
                                        ? commandMeta.stderr.split('\n').map((text) => ({
                                            stream: 'stderr' as const,
                                            text,
                                          }))
                                        : []),
                                    ].filter((entry) => entry.text.length > 0)
                                  : []

                              const dispatchMeta =
                                isTaskDispatchFailed &&
                                log.meta &&
                                typeof log.meta === 'object'
                                  ? (log.meta as { [key: string]: unknown })
                                  : null
                              const dispatchSource =
                                dispatchMeta && typeof dispatchMeta.source === 'string'
                                  ? dispatchMeta.source
                                  : null
                              const dispatchKind =
                                dispatchMeta && typeof dispatchMeta.kind === 'string'
                                  ? dispatchMeta.kind
                                  : null
                              const dispatchError =
                                dispatchMeta && typeof dispatchMeta.error === 'string'
                                  ? dispatchMeta.error
                                  : null

                              const cardVariantClass = isTaskDispatchFailed
                                ? 'border-error/70 bg-error/5'
                                : isImagePrune
                                ? log.level === 'warning'
                                  ? 'border-warning/70 bg-warning/5'
                                  : 'border-info/60 bg-info/5'
                                : isAutoUpdateRunUnknown
                                ? 'border-warning/70 bg-warning/5'
                                : ''

                              return (
                                <div
                                  key={log.id}
                                  className={`flex flex-col gap-1 rounded border border-base-200 bg-base-100 px-2 py-1 text-[11px] ${cardVariantClass}`}
                                >
                                  <div className="flex flex-wrap items-center justify-between gap-2">
                                    <div className="flex items-center gap-2">
                                      <span className="flex items-center gap-1 font-mono text-[11px]">
                                        {isTaskDispatchFailed ? (
                                          <Icon
                                            icon="mdi:alert-octagon-outline"
                                            className="text-error text-sm"
                                          />
                                        ) : null}
                                        {isImagePrune ? (
                                          <Icon
                                            icon="mdi:trash-can-outline"
                                            className={
                                              log.level === 'warning'
                                                ? 'text-warning text-sm'
                                                : 'text-info text-sm'
                                            }
                                          />
                                        ) : null}
                                        {isAutoUpdateRunUnknown ? (
                                          <Icon
                                            icon="mdi:help-circle-outline"
                                            className="text-warning text-sm"
                                          />
                                        ) : null}
                                        <span>{log.action}</span>
                                      </span>
                                      <span
                                        className={`badge badge-xs ${statusBadgeClass(
                                          log.status,
                                          log.level,
                                        )}`}
                                      >
                                        {logStatusLabel(log)}
                                      </span>
                                      {log.unit ? (
                                        <span className="badge badge-ghost badge-xs">
                                          {log.unit}
                                        </span>
                                      ) : null}
                                    </div>
                                    <div className="flex items-center gap-2 text-[10px] text-base-content/60">
                                      {commandMeta?.exit ? (
                                        <span className="badge badge-outline badge-xs font-mono">
                                          {commandMeta.exit}
                                        </span>
                                      ) : null}
                                      <span>{formatTs(log.ts)}</span>
                                    </div>
                                  </div>
                                  <p className="text-[11px] text-base-content/80">
                                    {log.summary}
                                  </p>
                                  {!isTaskDispatchFailed ? (
                                    <TaskLogMetaDetails
                                      meta={log.meta}
                                      unitAlreadyShown={Boolean(log.unit)}
                                    />
                                  ) : null}
                                  {hasNoSummaryHint ? (
                                    <p className="text-[10px] text-warning">
                                      no JSONL summary found
                                    </p>
                                  ) : null}
                                  {isTaskDispatchFailed ? (
                                    <div className="space-y-0.5">
                                      <p className="text-[10px] text-error">
                                        任务派发失败（Task dispatch failed，任务未进入业务执行阶段）
                                      </p>
                                      {dispatchSource || dispatchKind || dispatchError ? (
                                        <div className="text-[10px] text-base-content/70">
                                          {dispatchSource ? (
                                            <span className="mr-2">
                                              source · {dispatchSource}
                                            </span>
                                          ) : null}
                                          {dispatchKind ? (
                                            <span className="mr-2">
                                              kind · {dispatchKind}
                                            </span>
                                          ) : null}
                                          {dispatchError ? (
                                            <div className="mt-0.5 break-words">
                                              error · {dispatchError}
                                            </div>
                                          ) : null}
                                        </div>
                                      ) : null}
                                    </div>
                                  ) : null}
                                  {isImagePrune ? (
                                    <p className="text-[10px] text-base-content/70">
                                      后台镜像清理（best-effort），失败仅作为告警，不会改变任务整体结果。
                                    </p>
                                  ) : null}
                                  {commandMeta ? (
                                    <div className="mt-1 border-t border-base-200 pt-1">
                                      <button
                                        type="button"
                                        className="flex w-full items-center justify-between text-[11px] text-base-content/70"
                                        onClick={() =>
                                          setExpandedCommandLogs((prev) => ({
                                            ...prev,
                                            [log.id]: !prev[log.id],
                                          }))
                                        }
                                      >
                                        <span className="flex items-center gap-1">
                                          <Icon
                                            icon={
                                              expandedCommandLogs[log.id]
                                                ? 'mdi:chevron-down'
                                                : 'mdi:chevron-right'
                                            }
                                            className="text-xs"
                                          />
                                          <span>命令输出</span>
                                        </span>
                                      </button>
                                      {expandedCommandLogs[log.id] ? (
                                        <div className="mt-1 space-y-1">
                                          {commandMeta.command ? (
                                            <div className="flex items-start gap-2">
                                              <code className="flex-1 overflow-x-auto rounded bg-base-200 px-2 py-1 font-mono text-[11px]">
                                                {commandMeta.command}
                                              </code>
                                              <button
                                                type="button"
                                                className="btn btn-ghost btn-xs"
                                                onClick={() =>
                                                  handleCopyCommand(commandMeta.command)
                                                }
                                              >
                                                <Icon
                                                  icon="mdi:content-copy"
                                                  className="text-base"
                                                />
                                              </button>
                                            </div>
                                          ) : null}
                                          {combinedLines.length > 0 ? (
                                            <div className="space-y-0.5">
                                              <span className="text-[10px] uppercase tracking-wide text-base-content/60">
                                                stdout / stderr
                                              </span>
                                              <div className="max-h-32 overflow-auto rounded bg-base-200 p-2 font-mono text-[11px]">
                                                {combinedLines.length === 0 ? (
                                                  <span className="text-[10px] text-base-content/60">
                                                    （无输出）
                                                  </span>
                                                ) : (
                                                  <div className="space-y-0.5">
                                                    {combinedLines.map(
                                                      (entry, index) => (
                                                        <div
                                                          key={`${log.id}-${entry.stream}-${index}`}
                                                          className="flex items-baseline gap-2"
                                                        >
                                                          <span
                                                            className={`badge badge-xs ${
                                                              entry.stream === 'stderr'
                                                                ? 'badge-error'
                                                                : 'badge-neutral'
                                                            }`}
                                                          >
                                                            {entry.stream === 'stderr'
                                                              ? 'ERR'
                                                              : 'OUT'}
                                                          </span>
                                                          <span className="text-[10px] text-base-content/60 tabular-nums">
                                                            {formatTimeWithMs(log.ts)}
                                                          </span>
                                                          <span className="flex-1 whitespace-pre-wrap break-words">
                                                            {entry.text}
                                                          </span>
                                                        </div>
                                                      ),
                                                    )}
                                                  </div>
                                                )}
                                              </div>
                                            </div>
                                          ) : null}
                                        </div>
                                      ) : null}
                                    </div>
                                  ) : null}
                                </div>
                              )
                            })}
                          </div>
                        </div>
                      )
                    })()}
                  </section>
                </>
              ) : null}
            </div>

            <div className="border-t border-base-200 px-4 py-2">
              {drawerTask ? (
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div className="flex flex-wrap items-center gap-2">
                    {drawerTask.status === 'running' && drawerTask.can_stop ? (
                      <button
                        type="button"
                        className="btn btn-xs btn-primary"
                        onClick={handleStop}
                      >
                        <Icon icon="mdi:stop-circle-outline" className="text-lg" />
                        停止
                      </button>
                    ) : null}
                    {drawerTask ? (
                      <button
                        type="button"
                        className="btn btn-xs btn-outline"
                        onClick={handleExport}
                      >
                        <Icon icon="mdi:download" className="text-lg" />
                        导出 JSON
                      </button>
                    ) : null}
                    {drawerTask.status === 'running' && drawerTask.can_force_stop ? (
                      <button
                        type="button"
                        className="btn btn-xs btn-outline btn-error"
                        onClick={handleForceStop}
                      >
                        <Icon icon="mdi:alert-octagon-outline" className="text-lg" />
                        强制停止
                      </button>
                    ) : null}
                    {drawerTask.status !== 'running' && drawerTask.can_retry ? (
                      <button
                        type="button"
                        className="btn btn-xs btn-outline"
                        onClick={handleRetry}
                      >
                        <Icon icon="mdi:restart" className="text-lg" />
                        重试
                      </button>
                    ) : null}
                    {drawerTask.events_hint?.task_id ? (
                      <Link
                        to={`/events?task_id=${encodeURIComponent(drawerTask.events_hint.task_id)}`}
                        className="btn btn-link btn-xs text-[11px]"
                      >
                        查看关联事件
                      </Link>
                    ) : null}
                  </div>
                  {mockEnabled ? (
                    <span className="text-[10px] text-base-content/60">
                      当前在 Mock 模式下，停止、强制停止和重试仅更新本地模拟数据，不会操作真实系统。
                    </span>
                  ) : null}
                </div>
              ) : null}
            </div>
          </div>
        </div>
      ) : null}
    </div>
  )
}
