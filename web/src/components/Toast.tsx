import { Icon } from '@iconify/react'
import type { PropsWithChildren } from 'react'
import { createContext, useCallback, useContext, useMemo, useState } from 'react'

export type ToastVariant = 'info' | 'success' | 'warning' | 'error'

export type ToastOptions = {
  id?: string
  variant?: ToastVariant
  title: string
  message?: string
}

type ToastContextValue = {
  pushToast: (toast: ToastOptions) => void
}

type ToastState = ToastOptions & { id: string }

const ToastContext = createContext<ToastContextValue | null>(null)

export function ToastProvider({ children }: PropsWithChildren) {
  const [toasts, setToasts] = useState<ToastState[]>([])

  const pushToast = useCallback((toast: ToastOptions) => {
    const id = toast.id ?? `${Date.now()}-${Math.random().toString(16).slice(2)}`
    const variant = toast.variant ?? 'info'
    const next: ToastState = { ...toast, id, variant }
    setToasts((prev) => [...prev, next])
    setTimeout(() => {
      setToasts((prev) => prev.filter((item) => item.id !== id))
    }, 5000)
  }, [])

  const value = useMemo(
    () => ({
      pushToast,
    }),
    [pushToast],
  )

  return (
    <ToastContext.Provider value={value}>
      {children}
      <div className="pointer-events-none fixed inset-x-0 bottom-4 z-40 flex justify-center">
        <div className="pointer-events-auto flex max-w-xl flex-col gap-2">
          {toasts.map((toast) => (
            <div
              key={toast.id}
              className={`alert shadow-lg ${
                toast.variant === 'success'
                  ? 'alert-success'
                  : toast.variant === 'warning'
                    ? 'alert-warning'
                    : toast.variant === 'error'
                      ? 'alert-error'
                      : 'alert-info'
              }`}
            >
              <Icon
                icon={
                  toast.variant === 'success'
                    ? 'mdi:check-circle'
                    : toast.variant === 'warning'
                      ? 'mdi:alert'
                      : toast.variant === 'error'
                        ? 'mdi:alert-circle'
                        : 'mdi:information-outline'
                }
                className="text-xl"
              />
              <div className="flex flex-1 flex-col">
                <span className="font-semibold">{toast.title}</span>
                {toast.message && (
                  <span className="text-xs opacity-80">{toast.message}</span>
                )}
              </div>
            </div>
          ))}
        </div>
      </div>
    </ToastContext.Provider>
  )
}

export function useToast(): ToastContextValue {
  const ctx = useContext(ToastContext)
  if (!ctx) {
    throw new Error('useToast must be used within ToastProvider')
  }
  return ctx
}

export function ToastViewport() {
  // viewport is rendered inside ToastProvider
  return null
}
