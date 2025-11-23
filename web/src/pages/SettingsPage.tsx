import { Icon } from '@iconify/react'
import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { useApi } from '../hooks/useApi'

type SettingsResponse = {
  env: {
    PODUP_STATE_DIR?: string
    PODUP_TOKEN_configured?: boolean
    PODUP_MANUAL_TOKEN_configured?: boolean
    PODUP_GH_WEBHOOK_SECRET_configured?: boolean
  }
  scheduler: {
    interval_secs?: number
    min_interval_secs?: number
    max_iterations?: number | null
  }
  systemd: {
    auto_update_unit?: string
    trigger_units?: string[]
  }
  database: {
    url?: string
  }
  version: {
    package?: string
    build_timestamp?: string | null
  }
  forward_auth: {
    header?: string | null
    admin_value_configured?: boolean
    nickname_header?: string | null
    admin_mode_name?: string | null
    dev_open_admin?: boolean
    mode?: string
  }
}

export default function SettingsPage() {
  const { getJson } = useApi()
  const [settings, setSettings] = useState<SettingsResponse | null>(null)

  useEffect(() => {
    let cancelled = false
    ;(async () => {
      try {
        const data = await getJson<SettingsResponse>('/api/settings')
        if (!cancelled) setSettings(data)
      } catch (err) {
        console.error('Failed to load settings', err)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [getJson])

  const scheduler = settings?.scheduler
  const systemd = settings?.systemd
  const forward = settings?.forward_auth

  return (
    <div className="space-y-6">
      <section className="card bg-base-100 shadow">
        <div className="card-body gap-3">
          <h2 className="text-lg font-semibold uppercase tracking-wide text-base-content/70">
            环境变量
          </h2>
          <div className="overflow-x-auto">
            <table className="table table-sm">
              <thead>
                <tr>
                  <th>变量</th>
                  <th>状态</th>
                  <th>值/说明</th>
                </tr>
              </thead>
              <tbody>
                <EnvRow
                  name="PODUP_STATE_DIR"
                  value={settings?.env.PODUP_STATE_DIR}
                  secret={false}
                />
                <EnvRow
                  name="PODUP_TOKEN"
                  configured={settings?.env.PODUP_TOKEN_configured}
                  secret
                />
                <EnvRow
                  name="PODUP_MANUAL_TOKEN"
                  configured={settings?.env.PODUP_MANUAL_TOKEN_configured}
                  secret
                />
                <EnvRow
                  name="PODUP_GH_WEBHOOK_SECRET"
                  configured={settings?.env.PODUP_GH_WEBHOOK_SECRET_configured}
                  secret
                />
              </tbody>
            </table>
          </div>
        </div>
      </section>

      <section className="grid gap-4 md:grid-cols-2">
        <div className="card bg-base-100 shadow">
          <div className="card-body gap-3">
            <h2 className="text-lg font-semibold uppercase tracking-wide text-base-content/70">
              Scheduler
            </h2>
            <ul className="space-y-1 text-xs text-base-content/80">
              <li>
                Interval: <code>{scheduler?.interval_secs ?? '--'}</code> seconds
              </li>
              <li>
                Min interval:{' '}
                <code>{scheduler?.min_interval_secs ?? '--'}</code> seconds
              </li>
              <li>
                Max iterations:{' '}
                <code>
                  {typeof scheduler?.max_iterations === 'number'
                    ? scheduler?.max_iterations
                    : '∞'}
                </code>
              </li>
            </ul>
          </div>
        </div>

        <div className="card bg-base-100 shadow">
          <div className="card-body gap-3">
            <h2 className="text-lg font-semibold uppercase tracking-wide text-base-content/70">
              systemd 单元
            </h2>
            <p className="text-xs text-base-content/70">
              auto-update unit 以及可通过 /api/manual 触发的业务单元。
            </p>
            <div className="space-y-2 text-xs">
              <div className="flex items-center gap-2">
                <span className="badge badge-outline badge-sm">auto-update</span>
                <span className="font-mono text-[11px]">
                  {systemd?.auto_update_unit ?? 'podman-auto-update.service'}
                </span>
              </div>
              <div className="space-y-1">
                {(systemd?.trigger_units ?? []).map((unit) => (
                  <div
                    key={unit}
                    className="flex items-center justify-between gap-2 rounded border border-base-200 px-2 py-1"
                  >
                    <span className="font-mono text-[11px]">{unit}</span>
                    <Link
                      to="/manual"
                      className="btn btn-ghost btn-xs gap-1"
                    >
                      <Icon icon="mdi:play-circle-outline" className="text-lg" />
                      手动触发
                    </Link>
                  </div>
                ))}
              </div>
            </div>
          </div>
        </div>
      </section>

      <section className="grid gap-4 md:grid-cols-2">
        <div className="card bg-base-100 shadow">
          <div className="card-body gap-3">
            <h2 className="text-lg font-semibold uppercase tracking-wide text-base-content/70">
              API & Version
            </h2>
            <ul className="space-y-1 text-xs text-base-content/80">
              <li>
                Database URL: <code>{settings?.database.url ?? '--'}</code>
              </li>
              <li>
                Package version: <code>{settings?.version.package ?? '--'}</code>
              </li>
              <li>
                Build time:{' '}
                <code>{settings?.version.build_timestamp ?? 'unknown'}</code>
              </li>
            </ul>
            <Link to="/events" className="btn btn-xs btn-outline gap-1">
              <Icon icon="mdi:file-document-outline" className="text-lg" />
              查看事件
            </Link>
          </div>
        </div>

        <div className="card bg-base-100 shadow">
          <div className="card-body gap-3">
            <h2 className="text-lg font-semibold uppercase tracking-wide text-base-content/70">
              ForwardAuth
            </h2>
            <ul className="space-y-1 text-xs text-base-content/80">
              <li>
                Header:{' '}
                <code>{forward?.header ?? '(not configured)'}</code>
              </li>
              <li>
                Admin value configured:{' '}
                <code>{forward?.admin_value_configured ? 'yes' : 'no'}</code>
              </li>
              <li>
                Nickname header:{' '}
                <code>{forward?.nickname_header ?? '(none)'}</code>
              </li>
              <li>
                Admin mode name:{' '}
                <code>{forward?.admin_mode_name ?? '(none)'}</code>
              </li>
              <li>
                DEV_OPEN_ADMIN:{' '}
                <code>{forward?.dev_open_admin ? 'true' : 'false'}</code>
              </li>
              <li>
                Mode: <code>{forward?.mode ?? 'open'}</code>
              </li>
            </ul>
          </div>
        </div>
      </section>
    </div>
  )
}

type EnvRowProps = {
  name: string
  value?: string
  configured?: boolean
  secret: boolean
}

function EnvRow({ name, value, configured, secret }: EnvRowProps) {
  const isConfigured =
    typeof configured === 'boolean'
      ? configured
      : typeof value === 'string' && value.length > 0

  return (
    <tr>
      <td className="font-mono text-xs">{name}</td>
      <td>
        <span
          className={`badge badge-xs ${
            isConfigured ? 'badge-success' : 'badge-warning'
          }`}
        >
          {isConfigured ? 'configured' : 'missing'}
        </span>
      </td>
      <td className="text-xs">
        {secret
          ? isConfigured
            ? '***'
            : '(empty)'
          : value ?? '(empty)'}
      </td>
    </tr>
  )
}
