import { expect, test } from '@playwright/test'

async function openManualPage(page: import('@playwright/test').Page) {
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

    await expect(page.getByText('有新版本')).toBeVisible()
    await expect(page.getByText('有更高版本')).toBeVisible()
    await expect(page.getByText('latest')).toBeVisible()

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

  test('clicking refresh button triggers refresh request', async ({ page }) => {
    await openManualPage(page)

    const refreshRequestPromise = page.waitForResponse(
      (res) =>
        res.url().includes('/api/manual/services') &&
        res.url().includes('refresh=1') &&
        res.status() === 200,
    )

    await page.getByRole('button', { name: '刷新更新状态' }).click()

    await refreshRequestPromise
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

  test('shows Manual token warning when required and hides it after input; 401 surfaces a specific toast', async ({ page }) => {
    await page.addInitScript(() => {
      window.localStorage.removeItem('webhook_manual_token')
    })
    await page.goto('/manual?mock=enabled&mock=profile=manual-token')

    const warning = page.getByText('当前环境已配置 Manual token', { exact: false })
    await expect(warning).toBeVisible()

    const tokenInput = page.getByPlaceholder('Manual token')
    await tokenInput.fill('wrong-secret')
    await expect(warning).toHaveCount(0)

    await page.route('**/api/manual/trigger', async (route) => {
      await route.fulfill({
        status: 401,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'manual token invalid' }),
      })
    })

    await page.getByRole('button', { name: '触发全部' }).click()

    await expect(
      page.getByText('Manual token 缺失或错误', { exact: false }),
    ).toBeVisible()
  })

  test('manual auto-update run succeeds in mock profile with correct Manual token', async ({ page }) => {
    await page.addInitScript(() => {
      window.localStorage.removeItem('webhook_manual_token')
    })
    await page.goto('/manual?mock=enabled&mock=profile=manual-token')

    const warning = page.getByText('当前环境已配置 Manual token', { exact: false })
    await expect(warning).toBeVisible()

    const tokenInput = page.getByPlaceholder('Manual token')
    await tokenInput.fill('mock-manual-token')

    // Warning should disappear once token is provided
    await expect(warning).toHaveCount(0)

    const row = page.locator('form', { hasText: 'podman-auto-update.service' }).first()

    // Use dry-run mode to avoid creating a long-running task in the mock runtime
    await row.getByLabel('Dry').check()
    await row.getByRole('button', { name: '触发' }).click()

    const response = await page.waitForResponse((res) =>
      res.url().includes('/api/manual/auto-update/run'),
    )
    expect(response.status()).toBeLessThan(400)

    // Should not show the previous failure/404-style toast
    await expect(page.getByText('Not Found')).toHaveCount(0)

    // Success/info toast should reflect auto-update start and dry-run status
    await expect(page.getByText('auto-update 执行已开始')).toBeVisible()
    await expect(
      page.getByText('podman-auto-update.service · status=dry-run'),
    ).toBeVisible()
  })

  test('manual auto-update run (non-dry) creates task and opens drawer in mock profile with correct Manual token', async ({ page }) => {
    await page.addInitScript(() => {
      window.localStorage.removeItem('webhook_manual_token')
    })
    await page.goto('/manual?mock=enabled&mock=profile=manual-token')

    const warning = page.getByText('当前环境已配置 Manual token', { exact: false })
    await expect(warning).toBeVisible()

    const tokenInput = page.getByPlaceholder('Manual token')
    await tokenInput.fill('mock-manual-token')
    await expect(warning).toHaveCount(0)

    const row = page.locator('form', { hasText: 'podman-auto-update.service' }).first()

    // Ensure non-dry run to create a real task
    await row.getByLabel('Dry').uncheck()
    await row.getByRole('button', { name: '触发' }).click()

    const response = await page.waitForResponse((res) =>
      res.url().includes('/api/manual/auto-update/run'),
    )
    expect(response.status()).toBeLessThan(400)

    await expect(page.getByText('Not Found')).toHaveCount(0)

    await expect(page.getByText('auto-update 执行已开始')).toBeVisible()
    await expect(
      page.getByText('podman-auto-update.service · status=pending'),
    ).toBeVisible()

    // Non-dry run returns task_id and should open the Manual task drawer
    await expect(page.getByText('任务中心')).toBeVisible()
  })

  test('shows meta.result_message details in Manual task drawer timeline (collapsible)', async ({ page }) => {
    await openManualPage(page)

    const svcRow = page.locator('form', { hasText: 'svc-alpha.service' }).first()
    await svcRow.getByLabel('Dry').uncheck()
    await svcRow.getByRole('button', { name: '触发' }).click()

    await expect(page.getByText('任务中心')).toBeVisible()

    const tasksListTab = page.getByRole('button', { name: '任务列表' })
    await expect(tasksListTab).toBeVisible()
    await tasksListTab.click()

    const listSection = page.locator('section').filter({ hasText: '任务列表' }).first()
    await expect(listSection).toBeVisible()

    const failingRow = listSection
      .locator('table tbody tr')
      .filter({
        hasText: 'Manual service failure demo · meta.result_message (svc-alpha)',
      })
      .first()
    await expect(failingRow).toBeVisible()
    await failingRow.click()

    const logsTimelineSection = page
      .locator('section')
      .filter({ hasText: '日志时间线' })
      .first()
    await expect(logsTimelineSection).toBeVisible()

    const manualServiceCard = logsTimelineSection
      .locator('div')
      .filter({ hasText: 'manual-service-run' })
      .filter({ hasText: 'Manual service task failed' })
      .first()
    await expect(manualServiceCard).toBeVisible()

    const expand = manualServiceCard.getByRole('button', { name: '展开详情' })
    await expect(expand).toBeVisible()
    await expand.click()

    await expect(
      manualServiceCard.getByText(
        'LAST_LINE: Failed to start svc-alpha.service: Permission denied',
      ),
    ).toBeVisible()
    await expect(manualServiceCard.getByText('result_status · failed')).toBeVisible()
  })
})
