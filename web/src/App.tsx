import { Icon } from '@iconify/react'
import { useEffect, useMemo, useState } from 'react'

type StreamStatus = 'connecting' | 'open' | 'error'

type StreamPayload = {
  message: string
  timestamp: number
}

const statusStyles: Record<StreamStatus, string> = {
  connecting: 'badge-info',
  open: 'badge-success',
  error: 'badge-error',
}

const statusLabel: Record<StreamStatus, string> = {
  connecting: 'Connecting to SSE channel…',
  open: 'Streaming heartbeat online',
  error: 'SSE channel unavailable',
}

const highlightCards = [
  {
    icon: 'mdi:lightning-bolt',
    title: 'Hello World SSE',
    body: 'EventSource wires the UI heartbeat to the webhook daemon.',
  },
  {
    icon: 'mdi:sprout',
    title: 'DaisyUI + Tailwind',
    body: 'Rely on DaisyUI design tokens for contrast and consistency.',
  },
  {
    icon: 'mdi:react',
    title: 'Vite + React + TS',
    body: 'Modern tooling keeps feedback loops under a second.',
  },
]

function useHelloStream() {
  const [status, setStatus] = useState<StreamStatus>('connecting')
  const [payload, setPayload] = useState<StreamPayload | null>(null)

  useEffect(() => {
    const source = new EventSource('/sse/hello')

    const onMessage = (event: MessageEvent<string>) => {
      try {
        const parsed = JSON.parse(event.data) as Partial<StreamPayload>
        setPayload({
          message: parsed.message ?? 'Webhook service ready',
          timestamp: parsed.timestamp ?? Math.floor(Date.now() / 1000),
        })
        setStatus('open')
      } catch (err) {
        console.error('Failed to parse SSE payload', err)
        setStatus('error')
      } finally {
        source.close()
      }
    }

    source.addEventListener('hello', onMessage)
    source.onmessage = onMessage
    source.onerror = () => {
      setStatus('error')
      source.close()
    }

    return () => {
      source.removeEventListener('hello', onMessage)
      source.close()
    }
  }, [])

  return { status, payload }
}

export default function App() {
  const { status, payload } = useHelloStream()

  const readableTime = useMemo(() => {
    if (!payload) {
      return '未收到心跳'
    }
    return new Date(payload.timestamp * 1000).toLocaleString()
  }, [payload])

  return (
    <div className="min-h-screen bg-base-200 text-base-content">
      <header className="navbar bg-base-100 shadow">
        <div className="navbar-start">
          <span className="btn btn-ghost text-xl font-title uppercase">Webhook Watcher</span>
        </div>
        <div className="navbar-end gap-3">
          <a className="btn btn-sm btn-outline" href="https://github.com/IvanLi-CN/codex-vibe-monitor" target="_blank" rel="noreferrer">
            <Icon icon="mdi:github" className="text-lg" /> Repo
          </a>
          <button className="btn btn-sm btn-primary text-primary-content" type="button">
            <Icon icon="mdi:cat" className="text-lg" /> Hello
          </button>
        </div>
      </header>

      <main className="mx-auto flex max-w-6xl flex-col gap-10 px-6 py-10">
        <section className="hero rounded-3xl border border-base-200 bg-base-100 shadow-2xl">
          <div className="hero-content w-full flex-col gap-10 lg:flex-row-reverse">
            <div className="card w-full max-w-md bg-base-200 text-base-content">
              <div className="card-body space-y-4">
                <div className="flex items-center justify-between">
                  <span className="text-sm font-semibold uppercase tracking-wide text-base-content">Realtime status</span>
                  <span className={`badge ${statusStyles[status]}`}>{statusLabel[status]}</span>
                </div>
                <p className="text-2xl font-semibold text-base-content">
                  {payload?.message ?? 'Listening for the server hello…'}
                </p>
                <div className="text-sm text-base-content">Last update · {readableTime}</div>
              </div>
            </div>
            <div className="flex-1 space-y-5">
              <span className="badge badge-secondary badge-outline">Hello world</span>
              <h1 className="font-title text-4xl font-semibold leading-tight text-base-content">
                Hello World · powered by <span className="text-primary">Vite + React + DaisyUI</span>
              </h1>
              <p className="text-lg text-base-content">
                Pure DaisyUI surface for webhook-auto-update. Strong contrast, responsive layout, and a friendly SSE heartbeat keep diagnostics simple.
              </p>
              <div className="join join-horizontal w-full flex-wrap">
                <a className="btn join-item btn-primary text-primary-content" href="https://github.com/IvanLi-CN/codex-vibe-monitor" target="_blank" rel="noreferrer">
                  <Icon className="text-2xl" icon="mdi:github" /> Reference Repo
                </a>
                <button className="btn join-item btn-outline" type="button">
                  <Icon className="text-2xl" icon="mdi:cat" /> Hello Webhook
                </button>
              </div>
            </div>
          </div>
        </section>

        <section className="stats stats-vertical shadow-lg lg:stats-horizontal bg-base-100 border border-base-200">
          <div className="stat">
            <div className="stat-figure text-primary">
              <Icon icon="mdi:access-point" className="text-2xl" />
            </div>
            <div className="stat-title">SSE channel</div>
            <div className="stat-value text-2xl">{status === 'open' ? 'Online' : 'Waiting'}</div>
            <div className="stat-desc">{statusLabel[status]}</div>
          </div>
          <div className="stat">
            <div className="stat-figure text-secondary">
              <Icon icon="mdi:clock-outline" className="text-2xl" />
            </div>
            <div className="stat-title">Last update</div>
            <div className="stat-value text-2xl">{payload ? new Date(payload.timestamp * 1000).toLocaleTimeString() : '--'}</div>
            <div className="stat-desc">{readableTime}</div>
          </div>
          <div className="stat">
            <div className="stat-figure text-accent">
              <Icon icon="mdi:react" className="text-2xl" />
            </div>
            <div className="stat-title">Stack</div>
            <div className="stat-value text-2xl">Vite · React</div>
            <div className="stat-desc">Typescript · DaisyUI · Iconify</div>
          </div>
        </section>

        <section className="grid gap-6 md:grid-cols-3">
          {highlightCards.map((card) => (
            <div key={card.title} className="card border border-base-200 bg-base-100 shadow-md text-base-content">
              <div className="card-body space-y-3">
                <div className="flex items-center gap-3 text-primary">
                  <Icon className="text-3xl" icon={card.icon} />
                  <h3 className="text-xl font-semibold">{card.title}</h3>
                </div>
                <p className="text-base">{card.body}</p>
              </div>
            </div>
          ))}
        </section>

        <section className="alert border border-base-200 bg-base-100 text-base-content">
          <Icon icon="mdi:information" className="text-3xl" />
          <div>
            <h3 className="font-semibold">Stack summary</h3>
            <div className="text-sm">
              Vite · React · TypeScript · DaisyUI · Iconify · SSE · Biome. Hello world only—extend routes and styles freely.
            </div>
          </div>
        </section>

        <footer className="pb-6 text-center text-sm text-base-content">
          Built with official DaisyUI components for consistent contrast.
        </footer>
      </main>
    </div>
  )
}
