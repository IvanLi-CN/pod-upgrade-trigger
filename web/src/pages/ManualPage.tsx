import { Icon } from '@iconify/react'
import type { FormEvent } from 'react'
import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useApi } from '../hooks/useApi'
import { useToast } from '../components/Toast'

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
        (r) => r.status === 'triggered' || r.status === 'dry-run',
      )
      pushToast({
        variant: ok ? 'success' : 'warning',
        title: ok ? '触发成功' : '部分失败',
        message: `触发 ${response.triggered.length} 个单元（dry_run=${response.dry_run}）。`,
      })
      pushHistory(response, `trigger-all (${response.triggered.length})`)

      if (!response.dry_run) {
        try {
          type CreateTaskResponse = {
            task_id: string
            is_long_running?: boolean
          }
          const task = await postJson<CreateTaskResponse>('/api/tasks', {
            kind: 'manual',
            source: 'manual',
            units: services.map((svc) => svc.unit),
            caller: body.caller ?? null,
            reason: body.reason ?? null,
            path: '/api/manual/trigger',
            is_long_running: true,
          })
          if (task?.task_id) {
            pushToast({
              variant: 'info',
              title: '已创建任务',
              message: '正在 /tasks 中跟踪本次触发。',
            })
            navigate(`/tasks?task_id=${encodeURIComponent(task.task_id)}`)
          }
        } catch {
          // 忽略任务创建失败，只保留原有触发结果
        }
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
      const response = await postJson<Record<string, unknown>>(
        `/api/manual/services/${encodeURIComponent(service.slug)}`,
        {
          dry_run: params.dryRun,
          image: params.image || undefined,
          caller: params.caller?.trim() || undefined,
          reason: params.reason?.trim() || undefined,
        },
      )
      const ok =
        response?.status === 'triggered' || response?.status === 'dry-run'
      pushToast({
        variant: ok ? 'success' : 'warning',
        title: ok ? '单元触发成功' : '单元触发失败',
        message: `${service.unit} · status=${response?.status ?? 'unknown'}`,
      })
      pushHistory(response, `trigger-unit ${service.unit}`)

      if (!params.dryRun) {
        try {
          type CreateTaskResponse = {
            task_id: string
            is_long_running?: boolean
          }
          const task = await postJson<CreateTaskResponse>('/api/tasks', {
            kind: 'manual',
            source: 'manual',
            units: [service.unit],
            caller: params.caller?.trim() || undefined,
            reason: params.reason?.trim() || undefined,
            path: `/api/manual/services/${service.slug}`,
            is_long_running: true,
          })
          if (task?.task_id) {
            pushToast({
              variant: 'info',
              title: '已创建任务',
              message: '正在 /tasks 中跟踪本次触发。',
            })
            navigate(`/tasks?task_id=${encodeURIComponent(task.task_id)}`)
          }
        } catch {
          // 忽略任务创建失败，只保留原有触发结果
        }
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
          <div className="grid gap-3 md:grid-cols-3">
            <label className="form-control">
              <span className="label-text text-xs">Caller</span>
              <input
                className="input input-sm input-bordered"
                value={allCaller}
                onChange={(event) => setAllCaller(event.target.value)}
                placeholder="who is triggering"
              />
            </label>
            <label className="form-control md:col-span-2">
              <span className="label-text text-xs">Reason</span>
              <input
                className="input input-sm input-bordered"
                value={allReason}
                onChange={(event) => setAllReason(event.target.value)}
                placeholder="short free-form reason"
              />
            </label>
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
