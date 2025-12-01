import { Icon } from '@iconify/react'
import type { FormEvent } from 'react'
import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useApi } from '../hooks/useApi'
import { useToast } from '../components/Toast'
import type { Task, TaskStatus, TaskDetailResponse, TasksListResponse } from '../domain/tasks'

type ManualService = {
  slug: string
  unit: string
  display_name: string
  default_image?: string | null
  github_path?: string
}

type ManualServicesResponse = {
  services: ManualService[]
}

type UnitActionResult = {
  unit: string
  status: string
  message?: string
}

type ManualTriggerResponse = {
  triggered: UnitActionResult[]
  dry_run: boolean
  caller?: string | null
  reason?: string | null
  task_id?: string | null
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
  const navigate = useNavigate()

  useEffect(() => {
    let cancelled = false
    ;(async () => {
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

  const handleTriggerAll = async (event: FormEvent) => {
    event.preventDefault()
    try {
      const body = {
        all: true,
        dry_run: allDryRun,
        caller: allCaller.trim() || undefined,
        reason: allReason.trim() || undefined,
      }
      const response = await postJson<ManualTriggerResponse>('/api/manual/trigger', body)
      const ok = response.triggered.every(
        (r) =>
          r.status === 'triggered' ||
          r.status === 'dry-run' ||
          r.status === 'pending',
      )
      pushToast({
        variant: ok ? 'success' : 'warning',
        title: ok ? '触发成功' : '部分失败',
        message: `触发 ${response.triggered.length} 个单元（dry_run=${response.dry_run}）。`,
      })
      pushHistory(response, `trigger-all (${response.triggered.length})`)

      if (!response.dry_run && response.task_id) {
        pushToast({
          variant: 'info',
          title: '已创建任务',
          message: '已在当前页面打开任务抽屉以跟踪本次触发。',
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
        title: '触发失败',
        message,
      })
    }
  }

  const handleTriggerService = async (
    service: ManualService,
    params: { dryRun: boolean; image?: string; caller?: string; reason?: string },
  ) => {
    try {
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
        title: ok ? '单元触发成功' : '单元触发失败',
        message: `${service.unit} · status=${response?.status ?? 'unknown'}`,
      })
      pushHistory(response, `trigger-unit ${service.unit}`)

      if (!params.dryRun && response.task_id) {
        pushToast({
          variant: 'info',
          title: '已创建任务',
          message: '已在当前页面打开任务抽屉以跟踪本次触发。',
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
        title: '单元触发失败',
        message,
      })
    }
  }

  const openHistoryInEvents = (entry: ManualHistoryEntry) => {
    if (entry.requestId) {
      navigate(`/events?request_id=${encodeURIComponent(entry.requestId)}`)
    }
  }

  return (
    <div className="space-y-6">
      <section className="card bg-base-100 shadow">
        <form className="card-body gap-4" onSubmit={handleTriggerAll}>
          <div className="flex items-center justify-between gap-4">
            <h2 className="text-lg font-semibold uppercase tracking-wide text-base-content/70">
              触发全部单元
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
              触发全部
            </button>
            <span className="text-[11px] text-base-content/60">
              映射到 POST /api/manual/trigger · all=true
            </span>
          </div>
        </form>
      </section>

      <section className="card bg-base-100 shadow">
        <div className="card-body gap-4">
          <div className="flex items-center justify-between">
            <h2 className="text-lg font-semibold uppercase tracking-wide text-base-content/70">
              按单元触发
            </h2>
            <span className="text-[11px] text-base-content/60">
              来自 GET /api/manual/services
            </span>
          </div>
          <div className="space-y-3">
            {services.length === 0 && (
              <p className="text-xs text-base-content/60">暂无可触发的 systemd 单元。</p>
            )}
            {services.map((service) => (
              <ServiceRow
                key={service.slug}
                service={service}
                onTrigger={handleTriggerService}
              />
            ))}
          </div>
        </div>
      </section>

      <section className="card bg-base-100 shadow">
        <div className="card-body gap-3">
          <div className="flex items-center justify-between">
            <h2 className="text-lg font-semibold uppercase tracking-wide text-base-content/70">
              历史记录
            </h2>
            <span className="text-[11px] text-base-content/60">
              最近 20 次触发，点击可跳转到 Events 视图
            </span>
          </div>
          <div className="space-y-2">
            {history.length === 0 && (
              <p className="text-xs text-base-content/60">暂无手动触发记录。</p>
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

type ServiceRowProps = {
  service: ManualService
  onTrigger: (
    service: ManualService,
    params: { dryRun: boolean; image?: string; caller?: string; reason?: string },
  ) => void | Promise<void>
}

function ServiceRow({ service, onTrigger }: ServiceRowProps) {
  const [image, setImage] = useState(service.default_image ?? '')
  const [caller, setCaller] = useState('')
  const [reason, setReason] = useState('')
  const [dryRun, setDryRun] = useState(false)
  const [pending, setPending] = useState(false)

  const handleSubmit = async (event: FormEvent) => {
    event.preventDefault()
    setPending(true)
    try {
      await onTrigger(service, {
        dryRun,
        image: image.trim() || undefined,
        caller,
        reason,
      })
    } finally {
      setPending(false)
    }
  }

  return (
    <form
      className="flex flex-col gap-2 rounded-lg border border-base-200 bg-base-100 px-3 py-2 text-xs md:flex-row md:items-center"
      onSubmit={handleSubmit}
    >
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <div className="flex items-center gap-2">
          <span className="font-semibold">{service.display_name}</span>
          <span className="badge badge-ghost badge-xs">{service.unit}</span>
        </div>
        <div className="grid gap-2 md:grid-cols-3">
          <input
            className="input input-xs input-bordered"
            placeholder={service.default_image ? 'override image' : 'image (optional)'}
            value={image}
            onChange={(event) => setImage(event.target.value)}
          />
          <input
            className="input input-xs input-bordered"
            placeholder="caller"
            value={caller}
            onChange={(event) => setCaller(event.target.value)}
          />
          <input
            className="input input-xs input-bordered"
            placeholder="reason"
            value={reason}
            onChange={(event) => setReason(event.target.value)}
          />
        </div>
        {service.github_path && (
          <div className="mt-1 flex items-center gap-1 text-[10px] text-base-content/60">
            <Icon icon="mdi:github" />
            <span>{service.github_path}</span>
          </div>
        )}
      </div>
      <div className="flex items-center gap-2 md:flex-col md:items-end">
        <label className="flex cursor-pointer items-center gap-1 text-[11px]">
          <span>Dry</span>
          <input
            type="checkbox"
            className="toggle toggle-xs"
            checked={dryRun}
            onChange={(event) => setDryRun(event.target.checked)}
          />
        </label>
        <button
          type="submit"
          className="btn btn-primary btn-xs"
          disabled={pending}
        >
          <Icon icon="mdi:play" className="text-lg" />
          触发
        </button>
      </div>
    </form>
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
  const [detailLoading, setDetailLoading] = useState(false)
  const [detailError, setDetailError] = useState<string | null>(null)

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

  const formatTs = (ts?: number | null) => {
    if (!ts || ts <= 0) return '--'
    return new Date(ts * 1000).toLocaleString()
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

  const statusBadgeClass = (status: TaskStatus) => {
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
      default:
        return 'badge-warning'
    }
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
                          <span
                            className={`badge badge-xs ${statusBadgeClass(task.status)}`}
                          >
                            {task.status}
                          </span>
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
                          {detail.status}
                        </span>
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
                                  {unit.status}
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
                    {detail.logs.length === 0 ? (
                      <p className="text-[11px] text-base-content/60">
                        暂无可用日志。
                      </p>
                    ) : (
                      <div className="space-y-1">
                        {detail.logs.map((log) => (
                          <div
                            key={log.id}
                            className="flex flex-col gap-1 rounded border border-base-200 bg-base-100 px-2 py-1 text-[11px]"
                          >
                            <div className="flex flex-wrap items-center justify-between gap-2">
                              <div className="flex items-center gap-2">
                                <span className="font-mono text-[11px]">
                                  {log.action}
                                </span>
                                <span
                                  className={`badge badge-xs ${statusBadgeClass(log.status)}`}
                                >
                                  {log.status}
                                </span>
                                {log.unit ? (
                                  <span className="badge badge-ghost badge-xs">
                                    {log.unit}
                                  </span>
                                ) : null}
                              </div>
                              <span className="text-[10px] text-base-content/60">
                                {formatTs(log.ts)}
                              </span>
                            </div>
                            <p className="text-[11px] text-base-content/80">
                              {log.summary}
                            </p>
                          </div>
                        ))}
                      </div>
                    )}
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
