import type { Meta, StoryObj } from '@storybook/react'
import { expect, within } from '@storybook/test'
import { ManualServicesCard } from './ManualServicesCard'

const meta: Meta<typeof ManualServicesCard> = {
  title: 'Components/Manual/ManualServicesCard',
  component: ManualServicesCard,
  tags: ['autodocs'],
}

export default meta
type Story = StoryObj<typeof ManualServicesCard>

const noop = async () => {}

export const Empty: Story = {
  args: {
    services: [],
    refreshing: false,
    onRefresh: noop,
    onTrigger: noop,
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement)
    expect(await canvas.findByText('暂无可部署的服务。')).toBeInTheDocument()
  },
}

export const Mixed: Story = {
  args: {
    services: [
      {
        slug: 'svc-1',
        unit: 'one.service',
        display_name: 'One',
        default_image: 'ghcr.io/acme/one:v1.0.0',
        update: { status: 'tag_update_available', tag: 'v1.0.1' },
      },
      {
        slug: 'svc-2',
        unit: 'two.service',
        display_name: 'Two',
        default_image: 'ghcr.io/acme/two:v2.0.0',
        update: { status: 'latest_ahead', tag: 'v2.0.0' },
      },
      {
        slug: 'svc-3',
        unit: 'three.service',
        display_name: 'Three',
        default_image: 'ghcr.io/acme/three:v3.0.0',
        update: { status: 'up_to_date', tag: 'v3.0.0' },
      },
      {
        slug: 'svc-4',
        unit: 'four.service',
        display_name: 'Four',
        default_image: 'ghcr.io/acme/four:v4.0.0',
        update: { status: 'unknown', tag: 'v4.0.0', reason: 'No remote digest' },
      },
    ],
    refreshing: false,
    onRefresh: noop,
    onTrigger: noop,
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement)
    expect(await canvas.findByText(/有新版本\s*v1\.0\.1/)).toBeInTheDocument()
    expect(await canvas.findByText(/有更高版本\s*latest/)).toBeInTheDocument()
    expect(await canvas.findByText('已是最新')).toBeInTheDocument()
    expect(await canvas.findByText('未知')).toBeInTheDocument()
  },
}

export const Refreshing: Story = {
  args: {
    services: [
      {
        slug: 'svc-1',
        unit: 'one.service',
        display_name: 'One',
        default_image: 'ghcr.io/acme/one:v1.0.0',
        update: { status: 'up_to_date', tag: 'v1.0.0' },
      },
    ],
    refreshing: true,
    onRefresh: noop,
    onTrigger: noop,
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement)
    const button = await canvas.findByRole('button', { name: '刷新更新状态' })
    expect(button).toBeDisabled()
    expect(button.querySelector('.animate-spin')).not.toBeNull()
  },
}

export const Loading: Story = {
  args: {
    services: [],
    refreshing: false,
    loading: true,
    onRefresh: noop,
    onTrigger: noop,
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement)
    expect(await canvas.findByText('正在加载服务列表…')).toBeInTheDocument()
  },
}
