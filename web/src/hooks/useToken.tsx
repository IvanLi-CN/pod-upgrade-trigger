import type { PropsWithChildren } from 'react'
import { createContext, useContext, useEffect, useState } from 'react'

type TokenContextValue = {
  token: string | null
  setToken: (token: string | null) => void
}

const STORAGE_KEY = 'webhook_manual_token'

const TokenContext = createContext<TokenContextValue | null>(null)

export function TokenProvider({ children }: PropsWithChildren) {
  const [token, setTokenState] = useState<string | null>(null)

  useEffect(() => {
    try {
      const stored = localStorage.getItem(STORAGE_KEY)
      if (stored) {
        setTokenState(stored)
      }
    } catch {
      // ignore storage errors
    }
  }, [])

  const setToken = (value: string | null) => {
    setTokenState(value)
    try {
      if (value) {
        localStorage.setItem(STORAGE_KEY, value)
      } else {
        localStorage.removeItem(STORAGE_KEY)
      }
    } catch {
      // ignore
    }
  }

  return (
    <TokenContext.Provider
      value={{
        token,
        setToken,
      }}
    >
      {children}
    </TokenContext.Provider>
  )
}

export function useToken(): TokenContextValue {
  const ctx = useContext(TokenContext)
  if (!ctx) {
    throw new Error('useToken must be used within TokenProvider')
  }
  return ctx
}
