import { setupWorker } from 'msw/browser'
import { handlers } from './handlers'
import { runtime } from './runtime'

export const worker = setupWorker(...handlers)

// Track which unhandled requests we've already warned about in this session.
const warnedUnhandled = new Set<string>()

export async function startMocks() {
  const url = new URL(window.location.href)
  const hasMockParam = url.searchParams.has('mock')
  const shouldRewriteUrl =
    import.meta.env.VITE_ENABLE_MOCKS !== 'true' && !hasMockParam

  if (shouldRewriteUrl) {
    url.searchParams.append('mock', 'enabled')
    window.history.replaceState(window.history.state, '', url.toString())
  }

  await worker.start({
    // Surface unhandled API requests at the mock layer without blocking
    // the actual network request. This helps spot missing handlers while
    // still allowing Vite/dev backends to serve the route.
    onUnhandledRequest(request, print) {
      try {
        const urlObj = new URL(request.url)
        const { pathname } = urlObj

        // Only care about API-style routes; let assets and other paths pass quietly.
        if (!pathname.startsWith('/api/')) {
          return
        }

        const key = `${request.method} ${pathname}`
        if (warnedUnhandled.has(key)) {
          return
        }
        warnedUnhandled.add(key)

        // One-time, non-fatal visibility into missing mocks.
        console.warn(
          '[msw-unhandled]',
          `Unhandled mock request for ${request.method} ${pathname}; request will bypass MSW and hit the real backend.`,
        )

        // Also print MSW's built-in warning for additional context.
        print.warning()
      } catch {
        // If anything goes wrong while logging, fall back to the default warning.
        print.warning()
      }
    },
  })

  window.__MOCK_ENABLED__ = true
  return runtime.snapshot()
}

declare global {
  interface Window {
    __MOCK_ENABLED__?: boolean
  }
}
