import { afterEach, describe, it } from 'vitest'
import { composeStories } from '@storybook/react'
import { cleanup, render } from '@testing-library/react'
import { expect, screen } from '@storybook/test'
import * as ManualUpdateBadgeStories from './ManualUpdateBadge.stories'

const { TagUpdateAvailable, LatestAhead, UpToDate, Unknown } = composeStories(
  ManualUpdateBadgeStories,
)

afterEach(() => cleanup())

describe('ManualUpdateBadge stories', () => {
  it('renders tag_update_available badge text', async () => {
    render(<TagUpdateAvailable />)
    expect(await screen.findByText('有新版本')).toBeInTheDocument()
    expect(await screen.findByText('v1.2.3')).toBeInTheDocument()
  })

  it('renders latest_ahead badge text', async () => {
    render(<LatestAhead />)
    expect(await screen.findByText('有更高版本')).toBeInTheDocument()
    expect(await screen.findByText('latest')).toBeInTheDocument()
  })

  it('renders up_to_date badge text', async () => {
    render(<UpToDate />)
    expect(await screen.findByText('已是最新')).toBeInTheDocument()
    expect(await screen.findByText('v1.2.3')).toBeInTheDocument()
  })

  it('renders unknown badge text with tooltip reason', async () => {
    render(<Unknown />)
    const badge = await screen.findByText('未知')
    expect(await screen.findByText('v1.2.3')).toBeInTheDocument()
    const tooltip = badge.closest('.tooltip')
    expect(tooltip).not.toBeNull()
    expect(tooltip).toHaveAttribute(
      'data-tip',
      expect.stringContaining('服务未返回远端 digest 信息'),
    )
  })
})
