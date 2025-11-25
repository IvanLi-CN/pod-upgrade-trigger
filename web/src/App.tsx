import { Icon } from '@iconify/react'
import { BrowserRouter, Link, Route, Routes, useLocation, useNavigate } from 'react-router-dom'
import DashboardPage from './pages/DashboardPage'
import EventsPage from './pages/EventsPage'
import ManualPage from './pages/ManualPage'
import MaintenancePage from './pages/MaintenancePage'
import SettingsPage from './pages/SettingsPage'
import WebhooksPage from './pages/WebhooksPage'
import UnauthorizedPage from './pages/UnauthorizedPage'
import { ApiProvider, useApi } from './hooks/useApi'
import { ToastProvider, ToastViewport } from './components/Toast'
import { TokenProvider, useToken } from './hooks/useToken'
import MockConsole from './mocks/MockConsole'

function TopStatusBar() {
  const { health, scheduler, sseStatus, now } = useAppStatus()
  const { token, setToken } = useToken()

  return (
    <header className="navbar sticky top-0 z-20 border-b border-base-300 bg-base-100/90 backdrop-blur">
      <div className="navbar-start gap-2 px-4">
        <span className="flex items-center gap-2 text-lg font-title font-semibold">
          <Icon icon="mdi:cat" className="text-2xl text-primary" />
          Webhook Control
        </span>
        <span className="badge badge-sm badge-outline hidden sm:inline-flex">
          {health === 'ok' ? 'Healthy' : health === 'error' ? 'Degraded' : 'Checking…'}
        </span>
      </div>
      <div className="navbar-center hidden md:flex">
        <div className="join">
          <span className="join-item badge badge-ghost gap-1">
            <Icon icon="mdi:timer-sand" className="text-lg" />
            {scheduler.intervalSecs}s
          </span>
          <span className="join-item badge badge-ghost gap-1">
            <Icon icon="mdi:autorenew" className="text-lg" />
            tick #{scheduler.lastIteration ?? '-'}
          </span>
          <span className="join-item badge badge-ghost gap-1">
            <Icon icon="mdi:access-point" className="text-lg" />
            {sseStatus === 'open' ? 'SSE ok' : sseStatus === 'error' ? 'SSE error' : 'SSE…'}
          </span>
        </div>
      </div>
      <div className="navbar-end gap-2 px-4">
        <form
          className="flex items-center gap-2"
          onSubmit={(event) => {
            event.preventDefault()
          }}
        >
          <label className="input input-sm input-bordered flex items-center gap-2 w-44">
            <Icon icon="mdi:key-variant" className="text-lg text-base-content/60" />
            <input
              className="min-w-0 flex-1 bg-transparent text-base"
              placeholder="Manual token"
              value={token ?? ''}
              onChange={(event) => setToken(event.target.value || null)}
            />
          </label>
        </form>
        <span className="hidden text-base text-base-content/70 sm:inline">
          {now.toLocaleTimeString()}
        </span>
      </div>
    </header>
  )
}

function SideNav() {
  const location = useLocation()
  const entries = [
    { to: '/', label: 'Dashboard', icon: 'mdi:view-dashboard' },
    { to: '/manual', label: 'Manual', icon: 'mdi:play-circle-outline' },
    { to: '/webhooks', label: 'Webhooks', icon: 'mdi:webhook' },
    { to: '/events', label: 'Events', icon: 'mdi:file-document-multiple-outline' },
    { to: '/maintenance', label: 'Maintenance', icon: 'mdi:toolbox-outline' },
    { to: '/settings', label: 'Settings', icon: 'mdi:cog-outline' },
  ]

  return (
    <aside className="h-full w-56 border-r border-base-300 bg-base-100/80 backdrop-blur">
      <nav className="flex h-full flex-col gap-2 p-3">
        <ul className="menu menu-sm flex-1 gap-1">
          {entries.map((entry) => {
            const active =
              entry.to === '/'
                ? location.pathname === '/'
                : location.pathname.startsWith(entry.to)
            return (
              <li key={entry.to}>
                <Link
                  to={entry.to}
                  className={active ? 'active font-semibold' : undefined}
                  aria-current={active ? 'page' : undefined}
                >
                  <Icon icon={entry.icon} className="text-lg" />
                  <span>{entry.label}</span>
                </Link>
              </li>
            )
          })}
        </ul>
        <div className="mt-auto flex flex-col gap-1 text-[11px] text-base-content/60">
          <span>Webhook auto-update UI</span>
        </div>
      </nav>
    </aside>
  )
}

function useAppStatus() {
  const { status } = useApi()
  return status
}

function Layout() {
  return (
    <div className="flex min-h-screen flex-col bg-base-200 text-base-content">
      <TopStatusBar />
      <div className="flex min-h-0 flex-1">
        <SideNav />
        <main className="flex-1 overflow-y-auto">
          <div className="mx-auto flex max-w-6xl flex-col gap-6 px-4 py-6">
            <Routes>
              <Route path="/" element={<DashboardPage />} />
              <Route path="/manual" element={<ManualPage />} />
              <Route path="/webhooks" element={<WebhooksPage />} />
              <Route path="/events" element={<EventsPage />} />
              <Route path="/maintenance" element={<MaintenancePage />} />
              <Route path="/settings" element={<SettingsPage />} />
              <Route path="/401" element={<UnauthorizedPage />} />
              <Route path="*" element={<NotFoundFallback />} />
            </Routes>
          </div>
        </main>
      </div>
      <ToastViewport />
    </div>
  )
}

function NotFoundFallback() {
  const navigate = useNavigate()
  return (
    <div className="hero min-h-[60vh]">
      <div className="hero-content text-center">
        <div className="max-w-md space-y-4">
          <h1 className="text-3xl font-bold">404 · 页面不存在</h1>
          <p className="text-lg text-base-content/70">
            所请求的路由不存在，可能是链接已失效或路径输入有误。
          </p>
          <button
            type="button"
            className="btn btn-primary btn-sm"
            onClick={() => navigate('/')}
          >
            返回 Dashboard
          </button>
        </div>
      </div>
    </div>
  )
}

type AppProps = {
  mockEnabled?: boolean
}

export default function App({ mockEnabled = false }: AppProps) {
  return (
    <BrowserRouter>
      <ToastProvider>
        <TokenProvider>
          <ApiProvider>
            <Layout />
          </ApiProvider>
        </TokenProvider>
      </ToastProvider>
      {mockEnabled ? <MockConsole /> : null}
    </BrowserRouter>
  )
}
