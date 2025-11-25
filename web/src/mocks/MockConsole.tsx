import { Icon } from '@iconify/react'
import { useEffect, useState } from 'react'
import type { MockProfile, RuntimeSnapshot } from './runtime'
import { runtime } from './runtime'

const profiles: MockProfile[] = [
  'happy-path',
  'empty-state',
  'rate-limit-hot',
  'auth-error',
  'degraded',
]

export function MockConsole() {
  const [open, setOpen] = useState(false)
  const [snapshot, setSnapshot] = useState<RuntimeSnapshot>(runtime.snapshot())

  useEffect(() => {
    const unsub = runtime.subscribe((snap) => setSnapshot(snap))
    return () => unsub()
  }, [])

  const handleProfileChange = (next: MockProfile) => {
    runtime.setProfile(next)
  }

  return (
    <div className="fixed bottom-4 right-4 z-50 flex flex-col items-end gap-2 text-xs">
      <button
        type="button"
        className="btn btn-xs btn-primary shadow-lg"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        aria-controls="mock-console"
      >
        <Icon icon="mdi:flask" className="text-lg" />
        Mock 控制台
      </button>

      {open && (
        <div
          id="mock-console"
          className="card w-80 max-w-[80vw] bg-base-100 shadow-2xl"
        >
          <div className="card-body gap-3">
            <div className="flex items-center justify-between">
              <span className="text-sm font-semibold">当前场景</span>
              <button
                type="button"
                className="btn btn-ghost btn-xs"
                onClick={() => runtime.resetData()}
              >
                重置数据
              </button>
            </div>
            <div className="grid grid-cols-2 gap-2">
              {profiles.map((profile) => {
                const active = snapshot.profile === profile
                return (
                  <button
                    key={profile}
                    type="button"
                    className={`btn btn-xs ${active ? 'btn-primary' : 'btn-outline'}`}
                    onClick={() => handleProfileChange(profile)}
                  >
                    {profile}
                  </button>
                )
              })}
            </div>

            <label className="form-control">
              <div className="label py-0">
                <span className="label-text text-[11px]">网络延迟 (ms)</span>
                <span className="label-text-alt text-[11px]">{snapshot.delayMs}ms</span>
              </div>
              <input
                type="range"
                min={0}
                max={2000}
                step={20}
                value={snapshot.delayMs}
                className="range range-xs"
                onChange={(event) => runtime.setDelayMs(Number(event.target.value))}
              />
            </label>

            <label className="form-control">
              <div className="label py-0">
                <span className="label-text text-[11px]">错误率</span>
                <span className="label-text-alt text-[11px]">
                  {(snapshot.errorRate * 100).toFixed(0)}%
                </span>
              </div>
              <input
                type="range"
                min={0}
                max={100}
                step={5}
                value={snapshot.errorRate * 100}
                className="range range-xs"
                onChange={(event) => runtime.setErrorRate(Number(event.target.value) / 100)}
              />
            </label>

            <div className="text-[11px] text-base-content/70">
              场景切换会重置数据；延迟与错误率仅影响 mock 响应，不会影响真实后端。
            </div>
          </div>
        </div>
      )}
    </div>
  )
}

export default MockConsole
