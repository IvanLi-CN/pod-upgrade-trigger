import { Icon } from '@iconify/react'
import { ManualServiceRow, type ManualServiceRowService, type ManualServiceRowTriggerParams } from './ManualServiceRow'

export type ManualServicesCardProps = {
  services: ManualServiceRowService[]
  refreshing: boolean
  onRefresh: () => void | Promise<void>
  onTrigger: (
    service: ManualServiceRowService,
    params: ManualServiceRowTriggerParams,
  ) => void | Promise<void>
}

export function ManualServicesCard({
  services,
  refreshing,
  onRefresh,
  onTrigger,
}: ManualServicesCardProps) {
  return (
    <section className="card bg-base-100 shadow">
      <div className="card-body gap-4">
        <div className="flex items-center justify-between">
          <h2 className="text-lg font-semibold uppercase tracking-wide text-base-content/70">
            按服务部署
          </h2>
          <div className="flex items-center gap-3">
            <button type="button" className="btn btn-xs" onClick={onRefresh} disabled={refreshing}>
              <span className={refreshing ? 'animate-spin' : ''}>
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
          {services.length === 0 && (
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
