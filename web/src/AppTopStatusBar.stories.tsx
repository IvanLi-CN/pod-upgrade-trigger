import type { Meta, StoryObj } from '@storybook/react'
import { expect, userEvent, within } from '@storybook/test'
import type { PropsWithChildren } from 'react'
import { useEffect, useState } from 'react'
import { BrowserRouter } from 'react-router-dom'
import { ToastProvider } from './components/Toast'
import { ApiProvider } from './hooks/useApi'
import { TopStatusBar } from './App'

function MockReady({ children }: PropsWithChildren) {
  const [ready, setReady] = useState(() => {
    if (typeof window === 'undefined') return false
    return (window as typeof window & { __MOCK_ENABLED__?: boolean }).__MOCK_ENABLED__ === true
  })

  useEffect(() => {
    if (ready) return
    let cancelled = false
    let timer: number | undefined

    const check = () => {
      if (cancelled) return
      const enabled = (window as typeof window & { __MOCK_ENABLED__?: boolean }).__MOCK_ENABLED__ === true
      if (enabled) {
        setReady(true)
        return
      }
      timer = window.setTimeout(check, 30)
    }

    check()

    return () => {
      cancelled = true
      if (timer !== undefined) window.clearTimeout(timer)
    }
  }, [ready])

  if (!ready) {
    return (
      <div className="p-6 text-sm text-base-content/70">
        Starting mock service worker…
      </div>
    )
  }

  return <>{children}</>
}

function TopStatusBarStory() {
  return (
    <MockReady>
      <BrowserRouter>
        <ToastProvider>
          <ApiProvider>
            <div className="min-h-[96px] bg-base-200">
              <TopStatusBar />
            </div>
          </ApiProvider>
        </ToastProvider>
      </BrowserRouter>
    </MockReady>
  )
}

const meta: Meta<typeof TopStatusBarStory> = {
  title: 'App/TopStatusBar',
  component: TopStatusBarStory,
  tags: ['autodocs'],
  parameters: {
    layout: 'fullscreen',
  },
}

export default meta
type Story = StoryObj<typeof TopStatusBarStory>

export const UpdateMenu: Story = {
  name: 'Update menu',
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement)
    const page = within(canvasElement.ownerDocument.body)

    expect(await canvas.findByText('Pod Upgrade Trigger', { exact: true })).toBeInTheDocument()
    expect(await canvas.findByText('v0.9.1', { exact: true })).toBeInTheDocument()

    const trigger = await canvas.findByRole('button', { name: '新版本菜单 v0.9.2' })
    await userEvent.click(trigger)

    expect(await page.findByRole('button', { name: '立即更新' })).toBeInTheDocument()
    const link = await page.findByRole('link', { name: '跳转到该版本代码页' })
    expect(link).toHaveAttribute(
      'href',
      'https://github.com/ivanli-cn/pod-upgrade-trigger/tree/v0.9.2',
    )
  },
}
