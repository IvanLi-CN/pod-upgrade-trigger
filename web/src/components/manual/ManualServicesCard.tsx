import { Icon } from '@iconify/react'
import { ManualServiceRow, type ManualServiceRowService, type ManualServiceRowTriggerParams } from './ManualServiceRow'

const SERVICE_SKELETON_KEYS = ['service-skeleton-1', 'service-skeleton-2', 'service-skeleton-3'] as const

export type ManualServicesCardProps = {
  services: ManualServiceRowService[]
  refreshing: boolean
  loading?: boolean
  error?: string | null
  onRefresh: () => void | Promise<void>
  onTrigger: (
    service: ManualServiceRowService,
    params: ManualServiceRowTriggerParams,
  ) => void | Promise<void>
}

export function ManualServicesCard({
  services,
  refreshing,
  loading,
  error,
  onRefresh,
  onTrigger,
}: ManualServicesCardProps) {
  const busy = Boolean(refreshing || loading)
  const showSkeleton = Boolean(services.length === 0 && busy)

  return (
    <section className="card bg-base-100 shadow">
      <div className="card-body gap-4">
        <div className="flex items-center justify-between">
          <h2 className="text-lg font-semibold uppercase tracking-wide text-base-content/70">
            按服务部署
          </h2>
          <div className="flex items-center gap-3">
            <button type="button" className="btn btn-xs" onClick={onRefresh} disabled={busy}>
              <span className={busy ? 'animate-spin' : ''}>
                <Icon icon="mdi:refresh" />
              </span>
              刷新更新状态
            </button>
            <span className="hidden text-[11px] text-base-content/60 sm:inline">
              来自 GET /api/manual/services
            </span>
          </div>
        </div>
        <div className="space-y-3">
          {error ? (
            <div className="alert alert-error py-2 text-xs">
              <Icon icon="mdi:alert-circle-outline" className="text-lg" />
              <span className="flex-1">{error}</span>
              <button type="button" className="btn btn-xs" onClick={onRefresh} disabled={busy}>
                重试
              </button>
            </div>
          ) : null}

          {showSkeleton ? (
            <>
              <output
                className="flex items-center gap-2 text-xs text-base-content/60"
                aria-live="polite"
              >
                <span className="loading loading-dots loading-xs" />
                <span>正在加载服务列表…</span>
              </output>
              {SERVICE_SKELETON_KEYS.map((key) => (
                <ManualServiceRowSkeleton key={key} />
              ))}
            </>
          ) : null}

          {!busy && !error && services.length === 0 && (
            <p className="text-xs text-base-content/60">暂无可部署的服务。</p>
          )}
          {services.map((service) => (
            <ManualServiceRow key={service.slug} service={service} onTrigger={onTrigger} />
          ))}
        </div>
      </div>
    </section>
  )
}

function ManualServiceRowSkeleton() {
  return (
    <div
      className="flex flex-col gap-2 rounded-lg border border-base-200 bg-base-100 px-3 py-2 text-xs md:flex-row md:items-center"
      aria-hidden="true"
    >
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <div className="flex items-center gap-2">
          <div className="skeleton h-4 w-32" />
          <div className="skeleton h-4 w-24" />
          <div className="skeleton h-4 w-12" />
          <div className="skeleton h-4 w-16" />
        </div>
        <div className="grid gap-2 md:grid-cols-3">
          <div className="skeleton h-7 w-full" />
          <div className="skeleton h-7 w-full" />
          <div className="skeleton h-7 w-full" />
        </div>
        <div className="mt-1 flex items-center gap-1 text-[10px] text-base-content/60">
          <div className="skeleton h-3 w-44" />
        </div>
      </div>
      <div className="flex items-center gap-2 md:flex-col md:items-end">
        <div className="skeleton h-4 w-14" />
        <div className="skeleton h-7 w-16" />
      </div>
    </div>
  )
}
