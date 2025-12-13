import type { JSX } from 'react'

export type ManualServiceUpdate = {
  status: 'tag_update_available' | 'latest_ahead' | 'up_to_date' | 'unknown'
  tag?: string
  running_digest?: string
  remote_tag_digest?: string
  remote_latest_digest?: string
  checked_at?: number
  stale?: boolean
  reason?: string
}

export function ManualUpdateBadge({
  update,
}: { update?: ManualServiceUpdate | null }): JSX.Element | null {
  if (!update) return null

  if (update.status === 'tag_update_available') {
    return <span className="badge badge-warning badge-sm">同 tag 有更新</span>
  }
  if (update.status === 'latest_ahead') {
    return <span className="badge badge-info badge-sm">latest 有变化</span>
  }
  if (update.status === 'up_to_date') {
    return <span className="badge badge-success badge-sm">已是最新</span>
  }

  return (
    <div className="tooltip" data-tip={update.reason || '未知原因'}>
      <span className="badge badge-ghost badge-sm border-base-content/20 text-base-content/50">
        未知
      </span>
    </div>
  )
}
