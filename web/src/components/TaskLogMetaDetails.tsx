import { useMemo, useState } from 'react'

type TaskLogMetaHints = {
  unit?: string
  image?: string | null
  result_status?: string
  result_message?: string
}

function parseMetaObject(meta: unknown): Record<string, unknown> | null {
  if (!meta) return null

  if (typeof meta === 'string') {
    try {
      const parsed = JSON.parse(meta) as unknown
      return parseMetaObject(parsed)
    } catch {
      return null
    }
  }

  if (typeof meta !== 'object') return null
  return meta as Record<string, unknown>
}

function readNonEmptyString(obj: Record<string, unknown>, key: string): string | null {
  const value = obj[key]
  if (typeof value !== 'string') return null
  const trimmed = value.trim()
  return trimmed.length > 0 ? trimmed : null
}

function extractMetaHints(meta: unknown): TaskLogMetaHints | null {
  const obj = parseMetaObject(meta)
  if (!obj) return null

  const unit = readNonEmptyString(obj, 'unit') ?? undefined
  const imageRaw = obj.image
  const image =
    imageRaw === null
      ? null
      : typeof imageRaw === 'string'
        ? imageRaw.trim() || null
        : null
  const result_status = readNonEmptyString(obj, 'result_status') ?? undefined
  const result_message = readNonEmptyString(obj, 'result_message') ?? undefined

  if (!unit && !image && !result_status && !result_message) return null
  return { unit, image, result_status, result_message }
}

function isLongMessage(message: string): boolean {
  const lineCount = message.split('\n').length
  return lineCount > 3 || message.length > 200
}

function buildCollapsedPreview(message: string): string {
  const lines = message.split('\n')
  if (lines.length > 3) {
    return `${lines.slice(0, 3).join('\n')}\n…`
  }
  if (message.length > 200) {
    return `${message.slice(0, 200)}…`
  }
  return message
}

export function TaskLogMetaDetails(props: {
  meta: unknown
  unitAlreadyShown?: boolean
}) {
  const { meta, unitAlreadyShown } = props
  const hints = useMemo(() => extractMetaHints(meta), [meta])
  const message = hints?.result_message ?? null
  const long = message ? isLongMessage(message) : false
  const [expanded, setExpanded] = useState(() => !long)

  const hintEntries = useMemo(() => {
    if (!hints) return []
    const entries: Array<{ key: string; value: string }> = []

    if (!unitAlreadyShown && hints.unit) {
      entries.push({ key: 'unit', value: hints.unit })
    }
    if (hints.image) {
      entries.push({ key: 'image', value: hints.image })
    }
    if (hints.result_status) {
      entries.push({ key: 'result_status', value: hints.result_status })
    }
    return entries
  }, [hints, unitAlreadyShown])

  if (!message && hintEntries.length === 0) return null

  return (
    <div className="mt-0.5 space-y-0.5">
      {message ? (
        <div className="rounded border border-base-200 bg-base-200/40 px-2 py-1">
          <div className="whitespace-pre-wrap break-words text-[11px] text-base-content/80">
            {long && !expanded ? buildCollapsedPreview(message) : message}
          </div>
          {long ? (
            <button
              type="button"
              className="btn btn-ghost btn-xs mt-1 h-auto min-h-0 px-1 py-0 text-[11px]"
              onClick={() => setExpanded((prev) => !prev)}
            >
              {expanded ? '收起详情' : '展开详情'}
            </button>
          ) : null}
        </div>
      ) : null}
      {hintEntries.length > 0 ? (
        <div className="flex flex-wrap gap-x-2 gap-y-0.5 text-[10px] text-base-content/60">
          {hintEntries.map((entry) => (
            <span key={entry.key}>
              {entry.key} · {entry.value}
            </span>
          ))}
        </div>
      ) : null}
    </div>
  )
}
