import type { Meta, StoryObj } from '@storybook/react'
import { expect, within } from '@storybook/test'
import { ManualServiceRow } from './ManualServiceRow'

const meta: Meta<typeof ManualServiceRow> = {
  title: 'Components/Manual/ManualServiceRow',
  component: ManualServiceRow,
  tags: ['autodocs'],
}

export default meta
type Story = StoryObj<typeof ManualServiceRow>

function expectCurrentVersionBetween({
  canvasElement,
  unit,
  currentTag,
  updateText,
}: {
  canvasElement: HTMLElement
  unit: string
  currentTag: string
  updateText?: string | RegExp
}) {
  const titleRow = within(canvasElement)
    .getByText('Demo service')
    .closest('div') as HTMLElement | null
  expect(titleRow).not.toBeNull()

  const row = within(titleRow as HTMLElement)
  const unitEl = row.getByText(unit)
  const currentEl = row.getByText(currentTag)
  expect(unitEl.compareDocumentPosition(currentEl) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()

  if (updateText) {
    const updateEl = row.getByText(updateText)
    expect(
      currentEl.compareDocumentPosition(updateEl) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy()
  }
}

const baseService = {
  slug: 'svc-demo',
  unit: 'demo.service',
  display_name: 'Demo service',
  default_image: 'ghcr.io/acme/demo:v1.2.3',
}

const noopTrigger = async () => {}

export const NoUpdate: Story = {
  args: {
    service: { ...baseService, update: null },
    onTrigger: noopTrigger,
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement)
    expect(await canvas.findByText('Demo service')).toBeInTheDocument()
    expect(await canvas.findByText('demo.service')).toBeInTheDocument()
    expectCurrentVersionBetween({
      canvasElement,
      unit: 'demo.service',
      currentTag: 'v1.2.3',
    })
    expect(canvas.queryByText('有新版本')).not.toBeInTheDocument()
    expect(canvas.queryByText('有更高版本')).not.toBeInTheDocument()
    expect(canvas.queryByText('已是最新')).not.toBeInTheDocument()
    expect(canvas.queryByText('未知')).not.toBeInTheDocument()
  },
}

export const TagUpdateAvailable: Story = {
  args: {
    service: {
      ...baseService,
      update: { status: 'tag_update_available', tag: 'v9.9.9' },
    },
    onTrigger: noopTrigger,
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement)
    expect(await canvas.findByText(/有新版本\s*v9\.9\.9/)).toBeInTheDocument()
    expectCurrentVersionBetween({
      canvasElement,
      unit: 'demo.service',
      currentTag: 'v9.9.9',
      updateText: /有新版本\s*v9\.9\.9/,
    })
  },
}

export const LatestAhead: Story = {
  args: {
    service: {
      ...baseService,
      update: { status: 'latest_ahead', tag: 'v1.2.3' },
    },
    onTrigger: noopTrigger,
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement)
    expect(await canvas.findByText(/有更高版本\s*latest/)).toBeInTheDocument()
    expectCurrentVersionBetween({
      canvasElement,
      unit: 'demo.service',
      currentTag: 'v1.2.3',
      updateText: /有更高版本\s*latest/,
    })
  },
}

export const UpToDate: Story = {
  args: {
    service: {
      ...baseService,
      update: { status: 'up_to_date', tag: 'v1.2.3' },
    },
    onTrigger: noopTrigger,
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement)
    expect(await canvas.findByText('已是最新')).toBeInTheDocument()
    expectCurrentVersionBetween({
      canvasElement,
      unit: 'demo.service',
      currentTag: 'v1.2.3',
      updateText: '已是最新',
    })
  },
}

export const Unknown: Story = {
  args: {
    service: {
      ...baseService,
      update: { status: 'unknown', tag: 'v1.2.3', reason: '服务未返回远端 digest 信息' },
    },
    onTrigger: noopTrigger,
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement)
    const badge = await canvas.findByText('未知')
    expectCurrentVersionBetween({
      canvasElement,
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
  },
}

export const WithGithubPath: Story = {
  args: {
    service: {
      ...baseService,
      github_path: 'acme/demo/services/demo.service',
      update: { status: 'up_to_date', tag: 'v1.2.3' },
    },
    onTrigger: noopTrigger,
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement)
    expect(await canvas.findByText('Demo service')).toBeInTheDocument()
    expect(await canvas.findByText('acme/demo/services/demo.service')).toBeInTheDocument()
    expect(await canvas.findByText('已是最新')).toBeInTheDocument()
    expectCurrentVersionBetween({
      canvasElement,
      unit: 'demo.service',
      currentTag: 'v1.2.3',
      updateText: '已是最新',
    })
  },
}

export const WithoutDefaultImage: Story = {
  args: {
    service: {
      ...baseService,
      default_image: null,
      update: { status: 'tag_update_available', tag: 'v9.9.9' },
    },
    onTrigger: noopTrigger,
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement)
    expect(await canvas.findByText('Demo service')).toBeInTheDocument()
    expect(await canvas.findByPlaceholderText('image (optional)')).toBeInTheDocument()
    expectCurrentVersionBetween({
      canvasElement,
      unit: 'demo.service',
      currentTag: 'v9.9.9',
      updateText: /有新版本\s*v9\.9\.9/,
    })
  },
}
