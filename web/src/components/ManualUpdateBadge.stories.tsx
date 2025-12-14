import type { Meta, StoryObj } from '@storybook/react'
import { expect, within } from '@storybook/test'
import { ManualUpdateBadge } from './ManualUpdateBadge'

const meta: Meta<typeof ManualUpdateBadge> = {
  title: 'Components/ManualUpdateBadge',
  component: ManualUpdateBadge,
  tags: ['autodocs'],
}

export default meta
type Story = StoryObj<typeof ManualUpdateBadge>

export const TagUpdateAvailable: Story = {
  args: {
    update: { status: 'tag_update_available', tag: 'v1.2.3' },
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement)
    expect(await canvas.findByText(/有新版本\s*v1\.2\.3/)).toBeInTheDocument()
  },
}

export const LatestAhead: Story = {
  args: {
    update: { status: 'latest_ahead', tag: 'v1.2.3' },
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement)
    expect(await canvas.findByText(/有更高版本\s*latest/)).toBeInTheDocument()
  },
}

export const UpToDate: Story = {
  args: {
    update: { status: 'up_to_date', tag: 'v1.2.3' },
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement)
    expect(await canvas.findByText('已是最新')).toBeInTheDocument()
  },
}

export const Unknown: Story = {
  args: {
    update: { status: 'unknown', tag: 'v1.2.3', reason: '服务未返回远端 digest 信息' },
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement)
    const badge = await canvas.findByText('未知')
    const tooltip = badge.closest('.tooltip')
    expect(tooltip).not.toBeNull()
    expect(tooltip).toHaveAttribute(
      'data-tip',
      expect.stringContaining('服务未返回远端 digest 信息'),
    )
  },
}
