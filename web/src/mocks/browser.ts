import { setupWorker } from 'msw/browser'
import { handlers } from './handlers'
import { runtime } from './runtime'

export const worker = setupWorker(...handlers)

export async function startMocks() {
  const url = new URL(window.location.href)
  const hasMockParam = url.searchParams.has('mock')
  const shouldRewriteUrl =
    import.meta.env.VITE_ENABLE_MOCKS !== 'true' && !hasMockParam

  if (shouldRewriteUrl) {
    url.searchParams.append('mock', 'enabled')
    window.history.replaceState(window.history.state, '', url.toString())
  }

  await worker.start({ onUnhandledRequest: 'bypass' })
  window.__MOCK_ENABLED__ = true
  return runtime.snapshot()
}

declare global {
  interface Window {
    __MOCK_ENABLED__?: boolean
  }
}
