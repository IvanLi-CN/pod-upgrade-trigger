import { Icon } from '@iconify/react'
import { useEffect } from 'react'
import { useLocation } from 'react-router-dom'

export default function UnauthorizedPage() {
  const location = useLocation() as ReturnType<typeof useLocation> & {
    state?: { originalPath?: string }
  }

  const originalPath =
    (location.state?.originalPath) || window.location.pathname

  useEffect(() => {
    try {
      window.history.replaceState(window.history.state, '', originalPath)
    } catch {
      // ignore
    }
  }, [originalPath])

  return (
    <div className="hero min-h-[60vh]">
      <div className="hero-content text-center">
        <div className="max-w-md space-y-4">
          <Icon icon="mdi:lock-alert" className="mx-auto text-5xl text-warning" />
          <h1 className="text-2xl font-semibold">未授权 · 401</h1>
          <p className="text-sm text-base-content/70">
            未登录或无权限访问该界面。请检查 ForwardAuth 或登录状态，刷新后重试。
          </p>
          <p className="text-xs text-base-content/60">
            当前请求路径：<code>{originalPath}</code>
          </p>
          <button
            type="button"
            className="btn btn-primary btn-sm mt-2"
            onClick={() => window.location.reload()}
          >
            刷新重试
          </button>
        </div>
      </div>
    </div>
  )
}
