import { afterEach, describe, it } from 'vitest'
import { composeStories } from '@storybook/react'
import { cleanup, render } from '@testing-library/react'
import { expect, screen } from '@storybook/test'
import * as ManualServicesCardStories from './ManualServicesCard.stories'

const { Empty, Mixed, Refreshing } = composeStories(ManualServicesCardStories)

afterEach(() => cleanup())

describe('ManualServicesCard stories', () => {
  it('renders empty message when no services', async () => {
    render(<Empty />)
    expect(await screen.findByText('暂无可触发的 systemd 单元。')).toBeInTheDocument()
  })

  it('renders mixed update badge states', async () => {
    render(<Mixed />)
    expect(await screen.findByText('有新版本')).toBeInTheDocument()
    expect(await screen.findByText('有更高版本')).toBeInTheDocument()
    expect(await screen.findByText('已是最新')).toBeInTheDocument()
    expect(await screen.findByText('未知')).toBeInTheDocument()
  })

  it('disables refresh button and shows animate-spin while refreshing', async () => {
    render(<Refreshing />)
    const button = await screen.findByRole('button', { name: '刷新更新状态' })
    expect(button).toBeDisabled()
    expect(button.querySelector('.animate-spin')).not.toBeNull()
  })
})

