import { Icon } from '@iconify/react'
import type { FormEvent } from 'react'
import { useState } from 'react'
import { ManualUpdateBadge, type ManualServiceUpdate } from '../ManualUpdateBadge'

export type ManualServiceRowService = {
  slug: string
  unit: string
  display_name: string
  default_image?: string | null
  github_path?: string
  is_auto_update?: boolean
  update?: ManualServiceUpdate | null
}

export type ManualServiceRowTriggerParams = {
  dryRun: boolean
  image?: string
  caller?: string
  reason?: string
}

export type ManualServiceRowProps = {
  service: ManualServiceRowService
  onTrigger: (
    service: ManualServiceRowService,
    params: ManualServiceRowTriggerParams,
  ) => void | Promise<void>
}

function extractTagFromImage(image?: string | null): string | null {
  const raw = image?.trim()
  if (!raw) return null

  const ref = raw.split('@')[0]?.trim()
  if (!ref) return null

  const lastSlash = ref.lastIndexOf('/')
  const lastColon = ref.lastIndexOf(':')
  if (lastColon <= lastSlash) return null

  const tag = ref.slice(lastColon + 1).trim()
  return tag ? tag : null
}

export function ManualServiceRow({ service, onTrigger }: ManualServiceRowProps) {
  const [image, setImage] = useState(service.default_image ?? '')
  const [caller, setCaller] = useState('')
  const [reason, setReason] = useState('')
  const [dryRun, setDryRun] = useState(false)
  const [pending, setPending] = useState(false)

  const currentTag = service.update?.tag?.trim() || extractTagFromImage(service.default_image)

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
          {currentTag ? (
            <span className="badge badge-ghost badge-xs text-base-content/60">{currentTag}</span>
          ) : null}
          <ManualUpdateBadge update={service.update} />
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
        <button type="submit" className="btn btn-primary btn-xs" disabled={pending}>
          <Icon icon="mdi:play" className="text-lg" />
          部署
        </button>
      </div>
    </form>
  )
}
