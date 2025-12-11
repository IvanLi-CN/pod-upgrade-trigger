import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useApi } from './useApi'

export type VersionInfo = {
  current?: { package?: string; releaseTag?: string | null }
  latest?: { releaseTag?: string | null; publishedAt?: string | null }
  hasUpdate?: boolean | null
  lastCheckedAt?: number | null
  loading: boolean
  error?: string | null
}

type VersionCheckResponse = {
  current?: { package?: string | null; release_tag?: string | null }
  latest?: { release_tag?: string | null; published_at?: string | null }
  has_update?: boolean | null
  checked_at?: number | null
  compare_reason?: string | null
}

const ONE_HOUR_MS = 60 * 60 * 1000
const LAST_CHECK_KEY = 'podup_version_last_check'
const LAST_TAG_KEY = 'podup_version_latest_tag'

function readNumber(key: string): number | null {
  try {
    const raw = localStorage.getItem(key)
    if (!raw) return null
    const value = Number(raw)
    return Number.isFinite(value) ? value : null
  } catch {
    return null
  }
}

function readString(key: string): string | null {
  try {
    const raw = localStorage.getItem(key)
    return raw ?? null
  } catch {
    return null
  }
}

function normalizeEpochMs(value?: number | null): number | null {
  if (typeof value !== 'number' || Number.isNaN(value)) return null
  // Backend likely returns seconds; treat < 1e12 as seconds.
  return value < 1_000_000_000_000 ? Math.round(value * 1000) : Math.round(value)
}

export function useVersionCheck(): VersionInfo {
  const { getJson } = useApi()

  const initialLastChecked = useMemo(() => readNumber(LAST_CHECK_KEY), [])
  const initialLatestTag = useMemo(() => readString(LAST_TAG_KEY), [])

  const [info, setInfo] = useState<VersionInfo>({
    current: undefined,
    latest: initialLatestTag ? { releaseTag: initialLatestTag, publishedAt: null } : undefined,
    hasUpdate: null,
    lastCheckedAt: initialLastChecked,
    loading: false,
    error: null,
  })

  const lastCheckedRef = useRef<number | null>(initialLastChecked ?? null)
  const checkingRef = useRef(false)
  const cancelledRef = useRef(false)

  useEffect(() => {
    return () => {
      cancelledRef.current = true
    }
  }, [])

  const persistSnapshot = useCallback((lastCheckedAt: number | null, latestTag: string | null) => {
    try {
      if (lastCheckedAt) {
        localStorage.setItem(LAST_CHECK_KEY, String(lastCheckedAt))
      }
      if (latestTag) {
        localStorage.setItem(LAST_TAG_KEY, latestTag)
      } else {
        localStorage.removeItem(LAST_TAG_KEY)
      }
    } catch {
      // ignore storage errors
    }
  }, [])

  const fetchVersion = useCallback(async () => {
    if (checkingRef.current) return
    checkingRef.current = true

    setInfo((prev) => ({
      ...prev,
      loading: true,
      error: null,
    }))

    try {
      const data = await getJson<VersionCheckResponse>('/api/version/check', {
        headers: { Accept: 'application/json' },
      })

      const latestTag = data.latest?.release_tag ?? null
      const normalizedCheckedAt = normalizeEpochMs(data.checked_at)
      const checkedAt = normalizedCheckedAt ?? Date.now()

      lastCheckedRef.current = checkedAt
      persistSnapshot(checkedAt, latestTag)

      if (cancelledRef.current) return

      setInfo({
        current: {
          package: data.current?.package ?? undefined,
          releaseTag: data.current?.release_tag ?? null,
        },
        latest: {
          releaseTag: latestTag,
          publishedAt: data.latest?.published_at ?? null,
        },
        hasUpdate: typeof data.has_update === 'boolean' ? data.has_update : null,
        lastCheckedAt: checkedAt,
        loading: false,
        error: null,
      })
    } catch (error) {
      const message =
        typeof error === 'object' && error && 'message' in error
          ? String((error as { message: unknown }).message)
          : 'version-check-failed'

      if (!cancelledRef.current) {
        setInfo((prev) => ({
          ...prev,
          loading: false,
          error: message,
        }))
      }
    } finally {
      checkingRef.current = false
    }
  }, [getJson, persistSnapshot])

  const maybeCheck = useCallback(() => {
    if (typeof document !== 'undefined' && document.visibilityState === 'hidden') {
      return
    }

    const last = lastCheckedRef.current
    const now = Date.now()
    if (last && now - last < ONE_HOUR_MS) {
      return
    }

    void fetchVersion()
  }, [fetchVersion])

  useEffect(() => {
    const onFocus = () => {
      maybeCheck()
    }

    const onVisibilityChange = () => {
      if (document.visibilityState === 'visible') {
        maybeCheck()
      }
    }

    window.addEventListener('focus', onFocus)
    document.addEventListener('visibilitychange', onVisibilityChange)

    // Initial check on mount if visible
    if (typeof document === 'undefined' || document.visibilityState === 'visible') {
      maybeCheck()
    }

    return () => {
      window.removeEventListener('focus', onFocus)
      document.removeEventListener('visibilitychange', onVisibilityChange)
    }
  }, [maybeCheck])

  return info
}
