import { afterEach, describe, it } from 'vitest'
import { composeStories } from '@storybook/react'
import { cleanup, render } from '@testing-library/react'
import { expect, screen, within } from '@storybook/test'
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

function expectCurrentVersionBetween({
  unit,
  currentTag,
  updateText,
}: {
  unit: string
  currentTag: string
  updateText?: string | RegExp
}) {
  const titleRow = screen.getByText('Demo service').closest('div') as HTMLElement | null
  expect(titleRow).not.toBeNull()

  const unitEl = within(titleRow as HTMLElement).getByText(unit)
  const currentEl = within(titleRow as HTMLElement).getByText(currentTag)
  expect(unitEl.compareDocumentPosition(currentEl) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()

  if (updateText) {
    const updateEl = within(titleRow as HTMLElement).getByText(updateText)
    expect(
      currentEl.compareDocumentPosition(updateEl) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy()
  }
}

describe('ManualServiceRow stories', () => {
  it('renders row without update badge', async () => {
    render(<NoUpdate />)
    expect(await screen.findByText('Demo service')).toBeInTheDocument()
    expect(await screen.findByText('demo.service')).toBeInTheDocument()
    expect(await screen.findByText('v1.2.3')).toBeInTheDocument()
    expectCurrentVersionBetween({ unit: 'demo.service', currentTag: 'v1.2.3' })
    expect(screen.queryByText('有新版本')).not.toBeInTheDocument()
    expect(screen.queryByText('有更高版本')).not.toBeInTheDocument()
    expect(screen.queryByText('已是最新')).not.toBeInTheDocument()
    expect(screen.queryByText('未知')).not.toBeInTheDocument()
  })

  it('renders tag_update_available badge', async () => {
    render(<TagUpdateAvailable />)
    expect(await screen.findByText(/有新版本\s*v9\.9\.9/)).toBeInTheDocument()
    expect(await screen.findByText('v9.9.9')).toBeInTheDocument()
    expectCurrentVersionBetween({
      unit: 'demo.service',
      currentTag: 'v9.9.9',
      updateText: /有新版本\s*v9\.9\.9/,
    })
  })

  it('renders latest_ahead badge', async () => {
    render(<LatestAhead />)
    expect(await screen.findByText(/有更高版本\s*latest/)).toBeInTheDocument()
    expect(await screen.findByText('v1.2.3')).toBeInTheDocument()
    expectCurrentVersionBetween({
      unit: 'demo.service',
      currentTag: 'v1.2.3',
      updateText: /有更高版本\s*latest/,
    })
  })

  it('renders up_to_date badge', async () => {
    render(<UpToDate />)
    expect(await screen.findByText('已是最新')).toBeInTheDocument()
    expect(await screen.findByText('v1.2.3')).toBeInTheDocument()
    expectCurrentVersionBetween({
      unit: 'demo.service',
      currentTag: 'v1.2.3',
      updateText: '已是最新',
    })
  })

  it('renders unknown badge with tooltip reason', async () => {
    render(<Unknown />)
    const badge = await screen.findByText('未知')
    expect(await screen.findByText('v1.2.3')).toBeInTheDocument()
    expectCurrentVersionBetween({
      unit: 'demo.service',
      currentTag: 'v1.2.3',
      updateText: '未知',
    })
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
    expect(await screen.findByText('v1.2.3')).toBeInTheDocument()
    expectCurrentVersionBetween({
      unit: 'demo.service',
      currentTag: 'v1.2.3',
      updateText: '已是最新',
    })
  })

  it('uses image (required) placeholder without default_image', async () => {
    render(<WithoutDefaultImage />)
    expect(await screen.findByPlaceholderText('image (required)')).toBeInTheDocument()
    expect(await screen.findByText('缺少镜像')).toBeInTheDocument()
    expect(await screen.findByText('v9.9.9')).toBeInTheDocument()
    expectCurrentVersionBetween({
      unit: 'demo.service',
      currentTag: 'v9.9.9',
      updateText: /有新版本\s*v9\.9\.9/,
    })
  })
})
