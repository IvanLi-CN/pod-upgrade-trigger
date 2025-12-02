import { expect, test } from '@playwright/test'

async function openManualPage(page: Parameters<typeof test>[0]['page']) {
  await page.goto('/manual?mock=enabled&mock=profile=happy-path')
  await expect(page.getByText('触发全部单元')).toBeVisible()
  await expect(page.getByText('按单元触发')).toBeVisible()
  await expect(page.getByText('历史记录')).toBeVisible()
}

test.describe('Manual triggers', () => {
  test('loads services and supports trigger-all dry-run', async ({ page }) => {
    await openManualPage(page)

    await expect(page.getByText('svc-alpha.service').first()).toBeVisible()
    await expect(page.getByText('svc-beta.service').first()).toBeVisible()

    await page.getByLabel('Dry run').check()

    await page.getByPlaceholder('who is triggering').fill('ui-e2e')
    await page.getByPlaceholder('short free-form reason').fill('trigger-all dry-run')

    await page.getByRole('button', { name: '触发全部' }).click()

    await expect(page.getByText('触发成功')).toBeVisible()

    await expect(page.getByText(/trigger-all \(\d+\)/)).toBeVisible()
  })

  test('supports per-service trigger with dry toggle', async ({ page }) => {
    await openManualPage(page)

    const row = page.locator('form', { hasText: 'svc-alpha.service' }).first()

    await row.getByPlaceholder(/image/).fill('ghcr.io/example/image:ui-e2e')
    await row.getByPlaceholder('caller').fill('ui-e2e')
    await row.getByPlaceholder('reason').fill('single-unit test')

    await row.getByLabel('Dry').check()

    await row.getByRole('button', { name: '触发' }).click()

    await expect(page.getByText('单元触发成功')).toBeVisible()
    await expect(page.getByText(/trigger-unit svc-alpha\.service/)).toBeVisible()
  })

  test('shows error toast when trigger-all fails', async ({ page }) => {
    await page.addInitScript(() => {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      ;(window as any).__MOCK_FORCE_MANUAL_FAILURE__ = true
    })

    await page.route('**/api/manual/trigger', async (route) => {
      await route.fulfill({
        status: 500,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'simulated failure' }),
      })
    })

    await openManualPage(page)

    await page.getByRole('button', { name: '触发全部' }).click()

    await expect(page.getByText('触发失败')).toBeVisible()
    await expect(page.getByText('暂无手动触发记录。')).toBeVisible()
  })

  test('history entry links to Events view with request_id filter', async ({ page }) => {
    await openManualPage(page)

    await page.getByLabel('Dry run').check()
    await page.getByPlaceholder('who is triggering').fill('ui-e2e-history')
    await page.getByPlaceholder('short free-form reason').fill('history-to-events')
    await page.getByRole('button', { name: '触发全部' }).click()

    await expect(page.getByText('触发成功')).toBeVisible()

    const historyEntry = page.getByRole('button', {
      name: /trigger-all \(\d+\)/,
    }).first()
    await expect(historyEntry).toBeVisible()

    await historyEntry.click()

    await expect(page).toHaveURL(/\/events\?request_id=/)
    await expect(page.getByText('事件与审计')).toBeVisible()

    const rows = page.locator('table tbody tr')
    await expect(rows.first()).toBeVisible()

    const reqCell = rows.first().locator('td').nth(1)
    const requestId = (await reqCell.textContent())?.trim() ?? ''
    expect(requestId).not.toEqual('')

    const filterValue = await page.getByLabel('Request ID').inputValue()
    expect(filterValue.trim()).toEqual(requestId)
  })
})
