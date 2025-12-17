import fs from 'node:fs'

import type { APIRequestContext } from '@playwright/test'
import { expect, test } from '@playwright/test'

let seeded = false

async function seedEvents(request: APIRequestContext) {
  if (seeded) return

  for (let i = 0; i < 3; i += 1) {
    await request.post('/api/manual/deploy', {
      headers: { 'Content-Type': 'application/json', 'x-podup-csrf': '1' },
      data: {
        all: true,
        dry_run: true,
        caller: `ui-e2e-${i}`,
        reason: 'events-seed',
      },
    })
  }

  seeded = true
}

test.describe('Events page', () => {
  test('lists events and supports filters', async ({ page, request }) => {
    await seedEvents(request)

    await page.goto('/events')

    await expect(page.getByText('事件与审计')).toBeVisible()

    const rows = page.locator('table tbody tr')
    await expect(rows.first()).toBeVisible()

    const firstRow = rows.first()
    const reqCell = firstRow.locator('td').nth(1)
    const statusCell = firstRow.locator('td').nth(4)
    const actionCell = firstRow.locator('td').nth(5)

    const requestId = (await reqCell.textContent())?.trim() ?? ''
    const status = (await statusCell.textContent())?.trim() ?? ''
    const action = (await actionCell.textContent())?.trim() ?? ''

    await page.getByLabel('Request ID').fill(requestId)

    await expect(rows.first().locator('td').nth(1)).toHaveText(requestId)

    await page.getByLabel('Status').fill(status)
    await expect(rows.first().locator('td').nth(4)).toContainText(status)

    await page.getByLabel('Status').fill('')
    await page.getByLabel('Action').fill(action)
    await expect(rows.first().locator('td').nth(5)).not.toBeEmpty()
  })

  test('shows details panel and exports CSV', async ({ page, request }) => {
    await seedEvents(request)

    await page.goto('/events')

    const rows = page.locator('table tbody tr')
    await expect(rows.first()).toBeVisible()

    await rows.first().click()

    await expect(page.getByText('详情')).toBeVisible()

    const pre = page.locator('pre')
    await expect(pre).toBeVisible()

    const [download] = await Promise.all([
      page.waitForEvent('download'),
      page.getByRole('button', { name: '导出当前页 CSV' }).click(),
    ])

    const downloadPath = await download.path()
    if (!downloadPath) {
      throw new Error('Download path is not available')
    }

    const content = fs.readFileSync(downloadPath, 'utf-8')
    const lines = content.trim().split('\n')
    expect(lines.length).toBeGreaterThan(1)
    expect(lines[0]).toContain('request_id')
  })
})
