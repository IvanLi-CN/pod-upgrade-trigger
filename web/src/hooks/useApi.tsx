import type { PropsWithChildren } from 'react'
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react'
import { useLocation, useNavigate } from 'react-router-dom'
import { useToast } from '../components/Toast'

type StreamStatus = 'idle' | 'connecting' | 'open' | 'error'

declare global {
  interface Window {
    __MOCK_ENABLED__?: boolean
  }
}

export type ApiError = {
  status: number
  message: string
}

export type SchedulerStatus = {
  intervalSecs: number
  lastIteration: number | null
}

export type AppStatus = {
  health: 'idle' | 'ok' | 'error'
  sseStatus: StreamStatus
  scheduler: SchedulerStatus
  now: Date
}

function isMockEnabled(): boolean {
  if (import.meta.env.VITE_ENABLE_MOCKS === 'true') {
    return true
  }

  if (typeof window === 'undefined') {
    return false
  }

  if (window.__MOCK_ENABLED__) {
    return true
  }

  return window.location.search.includes('mock')
}

type ApiContextValue = {
  status: AppStatus
  mockEnabled: boolean
  getJson: <T>(input: RequestInfo | URL, init?: RequestInit) => Promise<T>
  postJson: <T>(
    input: RequestInfo | URL,
    body: unknown,
    init?: RequestInit,
  ) => Promise<T>
}

const ApiContext = createContext<ApiContextValue | null>(null)

export function ApiProvider({ children }: PropsWithChildren) {
  const [health, setHealth] = useState<'idle' | 'ok' | 'error'>('idle')
  const [sseStatus, setSseStatus] = useState<StreamStatus>('idle')
  const [scheduler, setScheduler] = useState<SchedulerStatus>({
    intervalSecs: 900,
    lastIteration: null,
  })
  const [now, setNow] = useState<Date>(new Date())

  const navigate = useNavigate()
  const location = useLocation()
  const { pushToast } = useToast()
  const originalPathRef = useRef<string | null>(null)
  const mockEnabled = isMockEnabled()

  const handle401 = useCallback(() => {
    if (!originalPathRef.current) {
      originalPathRef.current = `${location.pathname}${location.search}`
    }

    const originalPath = originalPathRef.current
    if (import.meta.env.MODE === 'production' || mockEnabled) {
      navigate('/401', { replace: true, state: { originalPath } })
    } else {
      pushToast({
        variant: 'error',
        title: 'Unauthorized',
        message: 'Received 401 from backend. Check ForwardAuth configuration.',
      })
    }
  }, [location.pathname, location.search, mockEnabled, navigate, pushToast])

  useEffect(() => {
    let cancelled = false

    const probe = async () => {
      try {
        const res = await fetch('/health')
        if (res.status === 401) {
          handle401()
          return
        }
        if (cancelled) return
        setHealth(res.ok ? 'ok' : 'error')
      } catch {
        if (!cancelled) {
          setHealth('error')
        }
      }
    }

    const loadSettings = async () => {
      type SchedulerSnapshot = {
        interval_secs?: number
        recent_events?: { iteration?: number | null }[]
      }
      type SettingsSnapshot = {
        scheduler?: SchedulerSnapshot
      }

      try {
        const res = await fetch('/api/settings')
        if (res.status === 401) {
          handle401()
          return
        }
        if (!res.ok) return
        const data = (await res.json()) as SettingsSnapshot
        if (cancelled) return

        const intervalSecs = Number(data.scheduler?.interval_secs) || 900
        let lastIteration: number | null = null

        if (Array.isArray(data.scheduler?.recent_events)) {
          const latest = data.scheduler.recent_events.find(
            (entry) => typeof entry?.iteration === 'number',
          )
          if (latest && typeof latest.iteration === 'number') {
            lastIteration = latest.iteration
          }
        }

        setScheduler({ intervalSecs, lastIteration })
      } catch {
        // ignore
      }
    }

    probe()
    loadSettings()

    const timer = setInterval(() => {
      if (!cancelled) setNow(new Date())
    }, 1000)

    return () => {
      cancelled = true
      clearInterval(timer)
    }
  }, [handle401])

  useEffect(() => {
    let cancelled = false
    setSseStatus('connecting')
    // In mock mode, rely on a one-shot fetch to /sse/hello so degraded profile
    // still surfaces as error even if real EventSource is not available.
    if (mockEnabled) {
      ; (async () => {
        try {
          const res = await fetch('/sse/hello')
          if (cancelled) return
          setSseStatus(res.ok ? 'open' : 'error')
        } catch {
          if (!cancelled) setSseStatus('error')
        }
      })()
      return () => {
        cancelled = true
      }
    }

    const source = new EventSource('/sse/hello')

    const onMessage = () => {
      if (!cancelled) {
        setSseStatus('open')
      }
      source.close()
    }

    source.addEventListener('hello', onMessage)
    source.onmessage = onMessage
    source.onerror = () => {
      if (!cancelled) setSseStatus('error')
      source.close()
    }

    return () => {
      cancelled = true
      source.removeEventListener('hello', onMessage)
      source.close()
    }
  }, [mockEnabled])

  const getJson = useCallback(
    async <T,>(input: RequestInfo | URL, init?: RequestInit): Promise<T> => {
      const res = await fetch(input, {
        ...init,
        headers: {
          'Accept': 'application/json',
          ...(init?.headers ?? {}),
        },
      })

      if (res.status === 401) {
        handle401()
        throw {
          status: 401,
          message: 'unauthorized',
        } satisfies ApiError
      }

      if (!res.ok) {
        const text = await res.text().catch(() => res.statusText)
        throw {
          status: res.status,
          message: text || res.statusText,
        } satisfies ApiError
      }

      return (await res.json()) as T
    },
    [handle401],
  )

  const postJson = useCallback(
    async <T,>(input: RequestInfo | URL, body: unknown, init?: RequestInit): Promise<T> => {
      const headers: HeadersInit = {
        'Content-Type': 'application/json',
        'Accept': 'application/json',
        'X-Podup-CSRF': '1',
        ...(init?.headers ?? {}),
      }

      const res = await fetch(input, {
        ...init,
        method: 'POST',
        headers,
        body: JSON.stringify(body),
      })

      if (res.status === 401) {
        handle401()
        throw {
          status: 401,
          message: 'unauthorized',
        } satisfies ApiError
      }

      if (!res.ok) {
        const text = await res.text().catch(() => res.statusText)
        throw {
          status: res.status,
          message: text || res.statusText,
        } satisfies ApiError
      }

      return (await res.json()) as T
    },
    [handle401],
  )

  const value: ApiContextValue = useMemo(
    () => ({
      status: { health, sseStatus, scheduler, now },
      mockEnabled,
      getJson,
      postJson,
    }),
    [getJson, health, mockEnabled, now, scheduler, sseStatus, postJson],
  )

  return <ApiContext.Provider value={value}>{children}</ApiContext.Provider>
}

export function useApi(): ApiContextValue {
  const ctx = useContext(ApiContext)
  if (!ctx) {
    throw new Error('useApi must be used within ApiProvider')
  }
  return ctx
}
