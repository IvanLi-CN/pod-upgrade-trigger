import { afterEach, describe, it } from 'vitest'
import { composeStories } from '@storybook/react'
import { cleanup, render } from '@testing-library/react'
import { expect, screen } from '@storybook/test'
import * as ManualServiceRowStories from './ManualServiceRow.stories'

const {
  NoUpdate,
  TagUpdateAvailable,
  LatestAhead,
  UpToDate,
  Unknown,
  WithGithubPath,
  WithoutDefaultImage,
} = composeStories(ManualServiceRowStories)

afterEach(() => cleanup())

describe('ManualServiceRow stories', () => {
  it('renders row without update badge', async () => {
    render(<NoUpdate />)
    expect(await screen.findByText('Demo service')).toBeInTheDocument()
    expect(await screen.findByText('demo.service')).toBeInTheDocument()
    expect(screen.queryByText('有新版本')).not.toBeInTheDocument()
    expect(screen.queryByText('有更高版本')).not.toBeInTheDocument()
    expect(screen.queryByText('已是最新')).not.toBeInTheDocument()
    expect(screen.queryByText('未知')).not.toBeInTheDocument()
  })

  it('renders tag_update_available badge', async () => {
    render(<TagUpdateAvailable />)
    expect(await screen.findByText('有新版本')).toBeInTheDocument()
    expect(await screen.findByText('v9.9.9')).toBeInTheDocument()
  })

  it('renders latest_ahead badge', async () => {
    render(<LatestAhead />)
    expect(await screen.findByText('有更高版本')).toBeInTheDocument()
    expect(await screen.findByText('latest')).toBeInTheDocument()
  })

  it('renders up_to_date badge', async () => {
    render(<UpToDate />)
    expect(await screen.findByText('已是最新')).toBeInTheDocument()
    expect(await screen.findByText('v1.2.3')).toBeInTheDocument()
  })

  it('renders unknown badge with tooltip reason', async () => {
    render(<Unknown />)
    const badge = await screen.findByText('未知')
    const tooltip = badge.closest('.tooltip')
    expect(tooltip).not.toBeNull()
    expect(tooltip).toHaveAttribute(
      'data-tip',
      expect.stringContaining('服务未返回远端 digest 信息'),
    )
  })

  it('renders github_path line when present', async () => {
    render(<WithGithubPath />)
    expect(await screen.findByText('acme/demo/services/demo.service')).toBeInTheDocument()
  })

  it('uses image (optional) placeholder without default_image', async () => {
    render(<WithoutDefaultImage />)
    expect(await screen.findByPlaceholderText('image (optional)')).toBeInTheDocument()
  })
})

