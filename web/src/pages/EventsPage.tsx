import { Icon } from '@iconify/react'
import { useEffect, useMemo, useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import { useApi } from '../hooks/useApi'

type EventRecord = {
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

type EventsResponse = {
  events: EventRecord[]
  total: number
  page: number
  page_size: number
  has_next: boolean
}

export default function EventsPage() {
  const { getJson } = useApi()
  const [params, setParams] = useSearchParams()
  const [events, setEvents] = useState<EventRecord[]>([])
  const [total, setTotal] = useState(0)
  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(50)
  const [hasNext, setHasNext] = useState(false)
  const [selected, setSelected] = useState<EventRecord | null>(null)

  const filters = useMemo(
    () => ({
      requestId: params.get('request_id') ?? '',
      pathPrefix: params.get('path_prefix') ?? '',
      status: params.get('status') ?? '',
      action: params.get('action') ?? '',
    }),
    [params],
  )

  useEffect(() => {
    let cancelled = false
    const query = new URLSearchParams()
    query.set('page', String(page))
    query.set('per_page', String(pageSize))
    if (filters.requestId) query.set('request_id', filters.requestId)
    if (filters.pathPrefix) query.set('path_prefix', filters.pathPrefix)
    if (filters.status) query.set('status', filters.status)
    if (filters.action) query.set('action', filters.action)

    ;(async () => {
      try {
        const data = await getJson<EventsResponse>(`/api/events?${query.toString()}`)
        if (cancelled) return
        setEvents(data.events ?? [])
        setTotal(data.total ?? 0)
        setPage(data.page ?? page)
        setPageSize(data.page_size ?? pageSize)
        setHasNext(Boolean(data.has_next))
        if (data.events && data.events.length > 0 && !selected) {
          setSelected(data.events[0])
        }
      } catch (err) {
        console.error('Failed to load events', err)
      }
    })()

    return () => {
      cancelled = true
    }
  }, [
    filters.action,
    filters.pathPrefix,
    filters.requestId,
    filters.status,
    getJson,
    page,
    pageSize,
    selected,
  ])

  const updateFilter = (key: 'request_id' | 'path_prefix' | 'status' | 'action', value: string) => {
    const next = new URLSearchParams(params)
    if (value) {
      next.set(key, value)
    } else {
      next.delete(key)
    }
    setParams(next)
    setPage(1)
  }

  const formatTs = (ts: number) => {
    if (!ts || ts <= 0) return '--'
    return new Date(ts * 1000).toLocaleString()
  }

  const downloadCsv = () => {
    if (!events.length) return
    const header = ['id', 'request_id', 'ts', 'method', 'path', 'status', 'action', 'duration_ms']
    const rows = events.map((e) => [
      e.id,
      e.request_id,
      e.ts,
      e.method,
      e.path ?? '',
      e.status,
      e.action,
      e.duration_ms,
    ])
    const csv = [header.join(','), ...rows.map((row) => row.join(','))].join('\n')
    const blob = new Blob([csv], { type: 'text/csv;charset=utf-8;' })
    const url = URL.createObjectURL(blob)
    const link = document.createElement('a')
    link.href = url
    link.setAttribute('download', 'events-page.csv')
    document.body.appendChild(link)
    link.click()
    document.body.removeChild(link)
    URL.revokeObjectURL(url)
  }

  return (
    <div className="space-y-4">
      <section className="flex flex-wrap items-center justify-between gap-3">
        <div className="space-y-1">
          <h1 className="text-lg font-semibold">事件与审计</h1>
          <p className="text-xs text-base-content/70">
            数据来自 SQLite event_log 表，所有 HTTP / system 事件都会记录在此。
          </p>
        </div>
        <button
          type="button"
          className="btn btn-sm btn-outline gap-1"
          onClick={downloadCsv}
          disabled={!events.length}
        >
          <Icon icon="mdi:download" className="text-lg" />
          导出当前页 CSV
        </button>
      </section>

      <section className="card bg-base-100 shadow-sm">
        <div className="card-body gap-3">
          <div className="grid gap-2 md:grid-cols-4">
            <label className="form-control">
              <span className="label-text text-xs">Request ID</span>
              <input
                className="input input-xs input-bordered"
                value={filters.requestId}
                onChange={(event) => updateFilter('request_id', event.target.value)}
                placeholder="request id"
              />
            </label>
            <label className="form-control">
              <span className="label-text text-xs">Path prefix</span>
              <input
                className="input input-xs input-bordered"
                value={filters.pathPrefix}
                onChange={(event) => updateFilter('path_prefix', event.target.value)}
                placeholder="/api/manual"
              />
            </label>
            <label className="form-control">
              <span className="label-text text-xs">Status</span>
              <input
                className="input input-xs input-bordered"
                value={filters.status}
                onChange={(event) => updateFilter('status', event.target.value)}
                placeholder="200 / 500"
              />
            </label>
            <label className="form-control">
              <span className="label-text text-xs">Action</span>
              <input
                className="input input-xs input-bordered"
                value={filters.action}
                onChange={(event) => updateFilter('action', event.target.value)}
                placeholder="manual-trigger / github-webhook"
              />
            </label>
          </div>
          <div className="flex items-center justify-between text-[11px] text-base-content/60">
            <span>
              共 {total} 条 · 第 {page} 页
            </span>
            <div className="join">
              <button
                type="button"
                className="btn btn-xs join-item"
                disabled={page <= 1}
                onClick={() => setPage((p) => Math.max(1, p - 1))}
              >
                上一页
              </button>
              <button
                type="button"
                className="btn btn-xs join-item"
                disabled={!hasNext}
                onClick={() => setPage((p) => p + 1)}
              >
                下一页
              </button>
            </div>
          </div>
        </div>
      </section>

      <section className="grid gap-4 md:grid-cols-[3fr_2fr]">
        <div className="card bg-base-100 shadow-sm">
          <div className="card-body gap-2">
            <div className="overflow-x-auto">
              <table className="table table-xs">
                <thead>
                  <tr>
                    <th>Time</th>
                    <th>Req</th>
                    <th>Method</th>
                    <th>Path</th>
                    <th>Status</th>
                    <th>Action</th>
                  </tr>
                </thead>
                <tbody>
                  {events.map((event) => {
                    const selectedRow = selected?.id === event.id
                    return (
                      <tr
                        key={event.id}
                        className={selectedRow ? 'bg-base-200' : undefined}
                        onClick={() => setSelected(event)}
                      >
                        <td>{formatTs(event.ts)}</td>
                        <td className="font-mono text-[10px]">{event.request_id}</td>
                        <td>{event.method}</td>
                        <td className="max-w-xs truncate">{event.path ?? '-'}</td>
                        <td>
                          <span
                            className={`badge badge-xs ${
                              event.status >= 500
                                ? 'badge-error'
                                : event.status >= 400
                                  ? 'badge-warning'
                                  : 'badge-success'
                            }`}
                          >
                            {event.status}
                          </span>
                        </td>
                        <td>{event.action}</td>
                      </tr>
                    )
                  })}
                </tbody>
              </table>
            </div>
          </div>
        </div>

        <div className="card bg-base-100 shadow-sm">
          <div className="card-body gap-3">
            <h2 className="text-lg font-semibold uppercase tracking-wide text-base-content/70">
              详情
            </h2>
            {selected ? (
              <>
                <div className="space-y-1 text-xs">
                  <div className="flex items-center justify-between gap-2">
                    <span className="font-semibold">{selected.action}</span>
                    <span className="badge badge-ghost badge-xs">
                      {selected.method} {selected.status}
                    </span>
                  </div>
                  <div className="text-[11px] text-base-content/70">
                    {selected.path ?? '-'}
                  </div>
                  <div className="text-[11px] text-base-content/60">
                    {formatTs(selected.ts)} · req {selected.request_id} ·{' '}
                    {selected.duration_ms} ms
                  </div>
                </div>
                <div className="divider my-1" />
                <pre className="max-h-64 overflow-auto rounded-sm bg-base-200 p-2 text-[11px] leading-snug">
                  {JSON.stringify(selected.meta, null, 2)}
                </pre>
              </>
            ) : (
              <p className="text-xs text-base-content/60">
                选择左侧任意一行以查看详细元数据。
              </p>
            )}
          </div>
        </div>
      </section>
    </div>
  )
}
