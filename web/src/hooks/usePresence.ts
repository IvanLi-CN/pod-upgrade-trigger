import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

export type PresencePhase = 'closed' | 'entering' | 'open' | 'exiting'

type Options = {
  enterMs: number
  exitMs: number
}

type Presence = {
  present: boolean
  visible: boolean
  phase: PresencePhase
}

export function usePresence(open: boolean, options: Options): Presence {
  const { enterMs, exitMs } = options
  const [present, setPresent] = useState(open)
  const [visible, setVisible] = useState(false)
  const prevOpenRef = useRef(open)
  const exitTimeoutRef = useRef<number | null>(null)
  const rafRef = useRef<number | null>(null)

  const clearExitTimer = useCallback(() => {
    if (exitTimeoutRef.current !== null) {
      window.clearTimeout(exitTimeoutRef.current)
      exitTimeoutRef.current = null
    }
  }, [])

  const clearRaf = useCallback(() => {
    if (rafRef.current !== null) {
      window.cancelAnimationFrame(rafRef.current)
      rafRef.current = null
    }
  }, [])

  useEffect(() => {
    const prevOpen = prevOpenRef.current
    prevOpenRef.current = open

    if (open) {
      clearExitTimer()
      clearRaf()

      setPresent(true)

      if (enterMs <= 0) {
        setVisible(true)
        return
      }

      if (!prevOpen) {
        setVisible(false)
        rafRef.current = window.requestAnimationFrame(() => {
          setVisible(true)
          rafRef.current = null
        })
      } else {
        setVisible(true)
      }

      return
    }

    clearRaf()

    if (!present) return

    setVisible(false)
    clearExitTimer()

    if (exitMs <= 0) {
      setPresent(false)
      return
    }

    exitTimeoutRef.current = window.setTimeout(() => {
      setPresent(false)
      exitTimeoutRef.current = null
    }, exitMs)
  }, [enterMs, exitMs, open, present, clearExitTimer, clearRaf])

  useEffect(() => {
    return () => {
      clearExitTimer()
      clearRaf()
    }
  }, [clearExitTimer, clearRaf])

  const phase = useMemo<PresencePhase>(() => {
    if (!present && !open) return 'closed'
    if (!present && open) return 'entering'
    if (present && visible) return 'open'
    return open ? 'entering' : 'exiting'
  }, [open, present, visible])

  return { present, visible, phase }
}
