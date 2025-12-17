import { Icon } from '@iconify/react'
import type { FormEvent } from 'react'
import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useApi } from '../hooks/useApi'
import { useToast } from '../components/Toast'
import type {
  Task,
  TaskStatus,
  TaskDetailResponse,
  TasksListResponse,
  TaskLogEntry,
  TaskLogLevel,
} from '../domain/tasks'
import { isCommandMeta } from '../domain/tasks'
import { AutoUpdateWarningsBlock } from '../components/AutoUpdateWarningsBlock'
import { TaskLogMetaDetails } from '../components/TaskLogMetaDetails'
import { ManualServicesCard } from '../components/manual/ManualServicesCard'
import type { ManualServiceRowService } from '../components/manual/ManualServiceRow'

type ManualService = ManualServiceRowService

type ManualServicesResponse = {
  services: ManualService[]
}

type UnitActionResult = {
  unit: string
  status: string
  message?: string
}

type DeployActionResult = {
  unit: string
  image?: string | null
  status: string
  message?: string | null
}

type ManualDeployResponse = {
  deploying: DeployActionResult[]
  skipped: UnitActionResult[]
  dry_run: boolean
  caller?: string | null
  reason?: string | null
  task_id?: string | null
  request_id?: string | null
}

type ServiceTriggerResponse = {
  unit: string
  status: string
  message?: string | null
  dry_run?: boolean
  caller?: string | null
  reason?: string | null
  image?: string | null
  task_id?: string | null
  request_id?: string | null
}

type ManualHistoryEntry = {
  id: string
  requestId?: string
  when: Date
  summary: string
  detail: unknown
}

export default function ManualPage() {
  const { getJson, postJson } = useApi()
  const { pushToast } = useToast()
  const [services, setServices] = useState<ManualService[]>([])
  const [history, setHistory] = useState<ManualHistoryEntry[]>([])
  const [taskDrawerVisible, setTaskDrawerVisible] = useState(false)
  const [taskDrawerInitialTaskId, setTaskDrawerInitialTaskId] = useState<string | null>(null)
  const [allDryRun, setAllDryRun] = useState(false)
  const [allCaller, setAllCaller] = useState('')
  const [allReason, setAllReason] = useState('')
  const [autoUpdateDryRun, setAutoUpdateDryRun] = useState(false)
  const [autoUpdateCaller, setAutoUpdateCaller] = useState('')
  const [autoUpdateReason, setAutoUpdateReason] = useState('')
  const [autoUpdatePending, setAutoUpdatePending] = useState(false)
  const [refreshing, setRefreshing] = useState(false)
  const navigate = useNavigate()

  useEffect(() => {
    let cancelled = false
      ; (async () => {
        try {
          const data = await getJson<ManualServicesResponse>('/api/manual/services')
          if (!cancelled && Array.isArray(data.services)) {
            setServices(data.services)
          }
        } catch (err) {
          console.error('Failed to load services', err)
        }
      })()
    return () => {
      cancelled = true
    }
  }, [getJson])

  const pushHistory = (response: unknown, summary: string) => {
    const id = `${Date.now()}-${Math.random().toString(16).slice(2)}`
    const requestId =
      response &&
        typeof response === 'object' &&
        'request_id' in response &&
        typeof (response as { request_id: unknown }).request_id === 'string'
        ? (response as { request_id: string }).request_id
        : undefined
    const entry: ManualHistoryEntry = {
      id,
      requestId,
      when: new Date(),
      summary,
      detail: response,
    }
    setHistory((prev) => [entry, ...prev].slice(0, 20))
  }

  const handleDeployAll = async (event: FormEvent) => {
    event.preventDefault()
    try {
      const body = {
        all: true,
        dry_run: allDryRun,
        caller: allCaller.trim() || undefined,
        reason: allReason.trim() || undefined,
      }
      const response = await postJson<ManualDeployResponse>('/api/manual/deploy', body)
      const deployingCount = Array.isArray(response.deploying) ? response.deploying.length : 0
      const skippedCount = Array.isArray(response.skipped) ? response.skipped.length : 0

      pushToast({
        variant: deployingCount > 0 ? 'success' : 'warning',
        title: deployingCount > 0 ? '部署请求已提交' : 'No deployable services',
        message: `deploying=${deployingCount}, skipped=${skippedCount}, dry_run=${response.dry_run}`,
      })
      pushHistory(
        response,
        `deploy-all (deploying=${deployingCount}, skipped=${skippedCount}, dry_run=${response.dry_run})`,
      )

      if (!response.dry_run && response.task_id) {
        pushToast({
          variant: 'info',
          title: '已创建任务',
          message: '已在当前页面打开任务抽屉以跟踪本次部署。',
        })
        setTaskDrawerInitialTaskId(response.task_id)
        setTaskDrawerVisible(true)
      }
    } catch (error) {
      const message =
        error && typeof error === 'object' && 'message' in error && error.message
          ? String(error.message)
          : 'Unknown error'
      pushToast({
        variant: 'error',
        title: '部署失败',
        message,
      })
    }
  }

  const handleDeployService = async (
    service: ManualService,
    params: { dryRun: boolean; image?: string; caller?: string; reason?: string },
  ) => {
    try {
      if (service.is_auto_update === true) {
        pushToast({
          variant: 'warning',
          title: 'auto-update 不是服务部署目标',
          message: '请使用下方的 auto-update 卡片执行。',
        })
        return
      }

      const response = await postJson<ServiceTriggerResponse>(
        `/api/manual/services/${encodeURIComponent(service.slug)}`,
        {
          dry_run: params.dryRun,
          image: params.image || undefined,
          caller: params.caller?.trim() || undefined,
          reason: params.reason?.trim() || undefined,
        },
      )
      const ok =
        response?.status === 'triggered' ||
        response?.status === 'dry-run' ||
        response?.status === 'pending'
      pushToast({
        variant: ok ? 'success' : 'warning',
        title: ok ? '服务部署成功' : '服务部署失败',
        message: `${service.unit} · status=${response?.status ?? 'unknown'}`,
      })
      pushHistory(response, `deploy-service ${service.unit}`)

      if (!params.dryRun && response.task_id) {
        pushToast({
          variant: 'info',
          title: '已创建任务',
          message: '已在当前页面打开任务抽屉以跟踪本次部署。',
        })
        setTaskDrawerInitialTaskId(response.task_id)
        setTaskDrawerVisible(true)
      }
    } catch (error) {
      const message =
        error && typeof error === 'object' && 'message' in error && error.message
          ? String(error.message)
          : 'Unknown error'
      pushToast({
        variant: 'error',
        title: '服务部署失败',
        message,
      })
    }
  }

  const autoUpdateService = services.find((service) => service.is_auto_update === true) ?? null
  const deployServices = services.filter((service) => service.is_auto_update !== true)

  const handleRunAutoUpdate = async (event: FormEvent) => {
    event.preventDefault()
    if (!autoUpdateService) return
    setAutoUpdatePending(true)
    try {
      const response = await postJson<ServiceTriggerResponse>('/api/manual/auto-update/run', {
        dry_run: autoUpdateDryRun,
        caller: autoUpdateCaller.trim() || undefined,
        reason: autoUpdateReason.trim() || undefined,
      })

      const status = response?.status ?? 'unknown'
      const alreadyRunning = status === 'already-running'
      const ok =
        status === 'pending' ||
        status === 'triggered' ||
        status === 'dry-run' ||
        alreadyRunning

      pushToast({
        variant: alreadyRunning ? 'info' : ok ? 'success' : 'warning',
        title: alreadyRunning ? '已有 auto-update 在运行' : ok ? 'auto-update 执行已开始' : 'auto-update 执行失败',
        message: `${autoUpdateService.unit} · status=${status}`,
      })
      pushHistory(
        response,
        alreadyRunning
          ? `auto-update-already-running ${autoUpdateService.unit}`
          : `auto-update-run ${autoUpdateService.unit}`,
      )

      if (!autoUpdateDryRun && response.task_id) {
        pushToast({
          variant: 'info',
          title: alreadyRunning ? '正在跟踪现有任务' : '已创建任务',
          message: '已在当前页面打开任务抽屉以跟踪 auto-update 执行。',
        })
        setTaskDrawerInitialTaskId(response.task_id)
        setTaskDrawerVisible(true)
      }
    } catch (error) {
      const message =
        error && typeof error === 'object' && 'message' in error && error.message
          ? String(error.message)
          : 'Unknown error'
      pushToast({
        variant: 'error',
        title: 'auto-update 执行失败',
        message,
      })
    } finally {
      setAutoUpdatePending(false)
    }
  }

  const openHistoryInEvents = (entry: ManualHistoryEntry) => {
    if (entry.requestId) {
      navigate(`/events?request_id=${encodeURIComponent(entry.requestId)}`)
    }
  }

  const handleRefresh = async () => {
    setRefreshing(true)
    try {
      const data = await getJson<ManualServicesResponse>('/api/manual/services?refresh=1')
      if (Array.isArray(data.services)) {
        setServices(data.services)
      }
    } catch (err) {
      const message =
        err && typeof err === 'object' && 'message' in err && err.message
          ? String(err.message)
          : '刷新失败'
      pushToast({
        variant: 'error',
        title: '更新刷新失败',
        message,
      })
    } finally {
      setRefreshing(false)
    }
  }

  return (
    <div className="space-y-6">
      <section className="card bg-base-100 shadow">
        <form className="card-body gap-4" onSubmit={handleDeployAll}>
          <div className="flex items-center justify-between gap-4">
            <h2 className="text-lg font-semibold uppercase tracking-wide text-base-content/70">
              部署全部服务
            </h2>
            <label className="flex cursor-pointer items-center gap-2 text-xs">
              <span>Dry run</span>
              <input
                type="checkbox"
                className="toggle toggle-xs"
                checked={allDryRun}
                onChange={(event) => setAllDryRun(event.target.checked)}
              />
            </label>
          </div>
          <div className="grid gap-3 md:grid-cols-2 text-xs">
            <div className="flex flex-col gap-1">
              <span className="text-xs">Caller</span>
              <input
                className="input input-sm input-bordered"
                value={allCaller}
                onChange={(event) => setAllCaller(event.target.value)}
                placeholder="who is triggering"
              />
            </div>
            <div className="flex flex-col gap-1">
              <span className="text-xs">Reason</span>
              <input
                className="input input-sm input-bordered"
                value={allReason}
                onChange={(event) => setAllReason(event.target.value)}
                placeholder="short free-form reason"
              />
            </div>
          </div>
          <div className="flex items-center justify-between gap-3">
            <button type="submit" className="btn btn-primary btn-sm">
              <Icon icon="mdi:play-circle" className="text-lg" />
              部署全部服务
            </button>
            <span className="text-[11px] text-base-content/60">
              POST /api/manual/deploy · all=true（不包含 auto-update）
            </span>
          </div>
        </form>
      </section>

      {autoUpdateService ? (
        <section className="card bg-base-100 shadow">
          <form className="card-body gap-4" onSubmit={handleRunAutoUpdate}>
            <div className="flex items-center justify-between gap-4">
              <div className="flex min-w-0 flex-1 flex-col">
                <h2 className="text-lg font-semibold uppercase tracking-wide text-base-content/70">
                  运行 podman auto-update
                </h2>
                <span className="text-[11px] text-base-content/60">
                  {autoUpdateService.unit} · POST /api/manual/auto-update/run
                </span>
              </div>
              <label className="flex cursor-pointer items-center gap-2 text-xs">
                <span>Auto dry-run</span>
                <input
                  type="checkbox"
                  className="toggle toggle-xs"
                  checked={autoUpdateDryRun}
                  onChange={(event) => setAutoUpdateDryRun(event.target.checked)}
                />
              </label>
            </div>
            <div className="grid gap-3 md:grid-cols-2 text-xs">
              <div className="flex flex-col gap-1">
                <span className="text-xs">Caller</span>
                <input
                  className="input input-sm input-bordered"
                  value={autoUpdateCaller}
                  onChange={(event) => setAutoUpdateCaller(event.target.value)}
                  placeholder="who is running auto-update"
                />
              </div>
              <div className="flex flex-col gap-1">
                <span className="text-xs">Reason</span>
                <input
                  className="input input-sm input-bordered"
                  value={autoUpdateReason}
                  onChange={(event) => setAutoUpdateReason(event.target.value)}
                  placeholder="auto-update reason"
                />
              </div>
            </div>
            <div className="flex items-center justify-between gap-3">
              <button
                type="submit"
                className="btn btn-primary btn-sm"
                disabled={autoUpdatePending}
              >
                <Icon icon="mdi:play-circle" className="text-lg" />
                运行 auto-update
              </button>
              <span className="text-[11px] text-base-content/60">
                仅运行 podman auto-update，不是服务部署。
              </span>
            </div>
          </form>
        </section>
      ) : null}

      <ManualServicesCard
        services={deployServices}
        refreshing={refreshing}
        onRefresh={handleRefresh}
        onTrigger={handleDeployService}
      />

      <section className="card bg-base-100 shadow">
        <div className="card-body gap-3">
          <div className="flex items-center justify-between">
            <h2 className="text-lg font-semibold uppercase tracking-wide text-base-content/70">
              历史记录
            </h2>
            <span className="text-[11px] text-base-content/60">
              最近 20 次操作，点击可跳转到 Events 视图
            </span>
          </div>
          <div className="space-y-2">
            {history.length === 0 && (
              <p className="text-xs text-base-content/60">暂无手动部署记录。</p>
            )}
            {history.map((entry) => (
              <button
                key={entry.id}
                type="button"
                className="flex w-full items-center justify-between rounded-lg border border-base-200 bg-base-100 px-3 py-2 text-left text-xs hover:border-primary/60 hover:bg-base-200"
                onClick={() => openHistoryInEvents(entry)}
              >
                <div className="flex min-w-0 flex-1 flex-col">
                  <span className="font-semibold">{entry.summary}</span>
                  <span className="text-[10px] text-base-content/60">
                    {entry.when.toLocaleString()}
                    {entry.requestId ? ` · req ${entry.requestId}` : null}
                  </span>
                </div>
                <Icon icon="mdi:open-in-new" className="text-lg text-base-content/70" />
              </button>
            ))}
          </div>
        </div>
      </section>
      {taskDrawerVisible ? (
        <ManualTasksDrawer
          initialTaskId={taskDrawerInitialTaskId}
          onClose={() => {
            setTaskDrawerVisible(false)
            setTaskDrawerInitialTaskId(null)
          }}
        />
      ) : null}
    </div>
  )
}

type ManualTasksDrawerProps = {
  initialTaskId?: string | null
  onClose: () => void
}

const TASKS_PAGE_SIZE = 20
const TASKS_POLL_INTERVAL_MS = 7000
const TASK_DETAIL_POLL_INTERVAL_MS = 3000

function ManualTasksDrawer({ initialTaskId, onClose }: ManualTasksDrawerProps) {
  const { status: appStatus, getJson } = useApi()
  const { pushToast } = useToast()
  const [activeTab, setActiveTab] = useState<'list' | 'detail'>(
    initialTaskId ? 'detail' : 'list',
  )
  const [tasks, setTasks] = useState<Task[]>([])
  const [total, setTotal] = useState(0)
  const [page, setPage] = useState(1)
  const [hasNext, setHasNext] = useState(false)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(
    initialTaskId ?? null,
  )
  const [detail, setDetail] = useState<TaskDetailResponse | null>(null)
  const [detailLogs, setDetailLogs] = useState<TaskLogEntry[] | null>(null)
  const [detailLoading, setDetailLoading] = useState(false)
  const [detailError, setDetailError] = useState<string | null>(null)
  const [expandedCommandLogs, setExpandedCommandLogs] = useState<Record<number, boolean>>({})
  const detailStatus = detail?.status

  // Keep drawer focused on the latest task when parent updates initialTaskId
  useEffect(() => {
    if (!initialTaskId) return
    setSelectedTaskId(initialTaskId)
    setActiveTab('detail')
  }, [initialTaskId])

  useEffect(() => {
    let cancelled = false

    const load = async () => {
      setLoading(true)
      setError(null)
      try {
        const query = new URLSearchParams()
        query.set('page', String(page))
        query.set('per_page', String(TASKS_PAGE_SIZE))
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

    const interval = window.setInterval(load, TASKS_POLL_INTERVAL_MS)

    return () => {
      cancelled = true
      window.clearInterval(interval)
    }
  }, [getJson, page])

  useEffect(() => {
    if (!selectedTaskId) {
      setDetail(null)
      setDetailLogs(null)
      setDetailError(null)
      return
    }

    let cancelled = false
    let timeoutId: number | undefined

    const loadDetail = async () => {
      if (cancelled) return
      setDetailLoading(true)
      setDetailError(null)
      try {
        const data = await getJson<TaskDetailResponse>(
          `/api/tasks/${encodeURIComponent(selectedTaskId)}`,
        )
        if (cancelled) return
        setDetail(data)
        setDetailLogs(Array.isArray(data.logs) ? data.logs : [])
        if (data.status === 'running') {
          timeoutId = window.setTimeout(loadDetail, TASK_DETAIL_POLL_INTERVAL_MS)
        }
      } catch (err) {
        if (cancelled) return
        const message =
          err && typeof err === 'object' && 'message' in err && err.message
            ? String(err.message)
            : '加载任务详情失败'
        setDetailError(message)
      } finally {
        if (!cancelled) {
          setDetailLoading(false)
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
  }, [getJson, selectedTaskId])

  useEffect(() => {
    if (!selectedTaskId) return
    setExpandedCommandLogs({})
  }, [selectedTaskId])

  // 当首次看到命令型日志时，默认展开命令输出（避免任务执行太快导致永远折叠）。
  useEffect(() => {
    if (!detailLogs || detailLogs.length === 0) return
    setExpandedCommandLogs((prev) => {
      let changed = false
      const next = { ...prev }
      for (const log of detailLogs) {
        if (next[log.id] !== undefined) continue
        if (!isCommandMeta(log.meta)) continue
        next[log.id] = true
        changed = true
      }
      return changed ? next : prev
    })
  }, [detailLogs])

  useEffect(() => {
    if (!selectedTaskId) return
    if (detailStatus !== 'running') return
    if (typeof EventSource === 'undefined') return

    let cancelled = false
    const url = `/sse/task-logs?task_id=${encodeURIComponent(selectedTaskId)}`
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
        setDetailLogs((prev) => {
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
  }, [detailStatus, selectedTaskId])

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
    // For timeline entries we prefer the log level over the underlying status
    // when deciding badge colour, so that warning/error logs are visually
    // distinct even if the task as a whole succeeded.
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

  const pageLabel = (() => {
    if (!total || total <= TASKS_PAGE_SIZE) return `第 ${page} 页`
    const maxPage = Math.ceil(total / TASKS_PAGE_SIZE)
    return `第 ${page} / ${maxPage} 页`
  })()

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

  return (
    <div className="fixed inset-0 z-40 flex justify-end bg-base-300/40">
      <div className="flex h-full w-full max-w-4xl flex-col border-l border-base-300 bg-base-100 shadow-xl">
        <div className="flex items-center justify-between border-b border-base-200 px-4 py-3">
          <div className="flex items-center gap-2">
            <span className="text-sm font-semibold">任务中心</span>
          </div>
          <button
            type="button"
            className="btn btn-ghost btn-xs"
            onClick={onClose}
          >
            <Icon icon="mdi:close" className="text-lg" />
          </button>
        </div>

        <div className="border-b border-base-200 px-4 pt-2">
          <div className="tabs tabs-sm tabs-bordered">
            <button
              type="button"
              className={`tab ${activeTab === 'list' ? 'tab-active' : ''}`}
              onClick={() => setActiveTab('list')}
            >
              任务列表
            </button>
            <button
              type="button"
              className={`tab ${activeTab === 'detail' ? 'tab-active' : ''}`}
              onClick={() => setActiveTab('detail')}
              disabled={!selectedTaskId}
            >
              任务详情
            </button>
          </div>
        </div>

        <div className="flex-1 overflow-y-auto px-4 py-3 space-y-3">
          {activeTab === 'list' ? (
            <section className="space-y-3">
              <div className="flex items-center justify-between gap-3">
                <div className="space-y-1">
                  <h2 className="text-sm font-semibold uppercase tracking-wide text-base-content/70">
                    任务列表
                  </h2>
                  <p className="text-[11px] text-base-content/60">
                    数据来自 /api/tasks · 每页 {TASKS_PAGE_SIZE} 条
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
                        <td
                          colSpan={7}
                          className="py-6 text-center text-xs text-base-content/60"
                        >
                          正在加载任务…
                        </td>
                      </tr>
                    ) : null}
                    {!loading && tasks.length === 0 && !error ? (
                      <tr>
                        <td
                          colSpan={7}
                          className="py-6 text-center text-xs text-base-content/60"
                        >
                          当前没有任务记录。
                        </td>
                      </tr>
                    ) : null}
                    {tasks.map((task) => (
                      <tr
                        key={task.task_id}
                        className="cursor-pointer hover:bg-base-200"
                        onClick={() => {
                          setSelectedTaskId(task.task_id)
                          setActiveTab('detail')
                        }}
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
              </div>
            </section>
          ) : (
            <section className="space-y-3">
              {!selectedTaskId ? (
                <p className="text-xs text-base-content/60">
                  尚未选择任务，请在“任务列表”标签页中点击一行任务。
                </p>
              ) : null}

              {detailError ? (
                <div className="alert alert-error py-2 text-xs">
                  <Icon icon="mdi:alert-circle-outline" className="text-lg" />
                  <span>{detailError}</span>
                </div>
              ) : null}

              {detailLoading && !detail ? (
                <p className="text-xs text-base-content/60">正在加载任务详情…</p>
              ) : null}

              {detail ? (
                <>
                  <section className="space-y-2">
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <div className="flex items-center gap-2">
                        <span className="badge badge-ghost badge-xs">
                          {formatKindLabel(detail.kind)}
                        </span>
                        <span
                          className={`badge badge-xs ${statusBadgeClass(detail.status)}`}
                        >
                          {renderTaskStatusLabel(detail.status)}
                        </span>
                        {detail.status === 'unknown' ? (
                          <span className="text-[10px] text-warning">
                            Status unknown
                          </span>
                        ) : null}
                        {detail.has_warnings ? (
                          <span className="badge badge-warning badge-xs gap-1">
                            <Icon icon="mdi:alert-outline" className="text-[11px]" />
                            <span>{detail.warning_count ?? 0}</span>
                          </span>
                        ) : null}
                      </div>
                      <div className="flex flex-wrap items-center gap-2 text-[11px] text-base-content/60">
                        <span>创建 · {formatTs(detail.created_at)}</span>
                        <span>
                          起止 · {formatTs(detail.started_at ?? detail.created_at)} →{' '}
                          {formatTs(detail.finished_at ?? null)}
                        </span>
                        <span>耗时 · {formatDuration(detail)}</span>
                      </div>
                    </div>
                    <p className="text-xs text-base-content/70">
                      {detail.summary ?? '暂无摘要说明。'}
                    </p>
                    <div className="flex flex-wrap items-center gap-2 text-[11px] text-base-content/60">
                      <span>来源 · {detail.trigger.source}</span>
                      {detail.trigger.caller ? (
                        <span>caller · {detail.trigger.caller}</span>
                      ) : null}
                      {detail.trigger.reason ? (
                        <span>reason · {detail.trigger.reason}</span>
                      ) : null}
                      {detail.trigger.path ? (
                        <span className="max-w-xs truncate">
                          path · {detail.trigger.path}
                        </span>
                      ) : null}
                    </div>
                  </section>

                  <section className="space-y-2">
                    <h3 className="text-xs font-semibold uppercase tracking-wide text-base-content/70">
                      单元状态
                    </h3>
                    {detail.units.length === 0 ? (
                      <p className="text-[11px] text-base-content/60">
                        当前任务没有关联 unit。
                      </p>
                    ) : (
                      <div className="space-y-1">
                        {detail.units.map((unit) => (
                          <div
                            key={`${unit.unit}-${unit.slug ?? ''}`}
                            className="flex flex-col gap-1 rounded border border-base-200 bg-base-100 px-2 py-1 text-[11px]"
                          >
                            <div className="flex flex-wrap items-center justify-between gap-2">
                              <div className="flex flex-wrap items-center gap-2">
                                <span className="font-mono text-[11px]">
                                  {unit.unit}
                                </span>
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
                      const logs = detailLogs ?? detail.logs
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
                                ? log.meta
                                : null

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
                                                    {combinedLines.map((entry, index) => (
                                                      <div
                                                        key={`${log.id}-${entry.stream}-${index}`}
                                                        className="flex items-baseline gap-2"
                                                      >
                                                        <span
                                                          className={`badge badge-xs ${entry.stream === 'stderr'
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
                                                    ))}
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
            </section>
          )}
        </div>
      </div>
    </div>
  )
}
