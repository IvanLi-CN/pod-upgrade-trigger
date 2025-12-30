import { expect, test } from '@playwright/test'

async function openManualPage(page: import('@playwright/test').Page) {
  await page.goto('/services?mock=enabled&mock=profile=happy-path')
  await expect(page.getByRole('heading', { name: '部署全部服务' })).toBeVisible()
  await expect(page.getByText('按服务升级')).toBeVisible()
  await expect(page.getByText('历史记录')).toBeVisible()
}

test.describe('Services deploy console', () => {
  test('redirects legacy /manual to /services (preserves query + hash)', async ({ page }) => {
    await page.goto('/manual?mock=enabled&mock=profile=happy-path#baz')
    await expect(page).toHaveURL('/services?mock=enabled&mock=profile=happy-path#baz')
    await expect(page.getByText('404 · 页面不存在')).toHaveCount(0)
    await expect(page.getByRole('heading', { name: '部署全部服务' })).toBeVisible()
  })

  test('loads services and supports deploy-all dry-run', async ({ page }) => {
    await openManualPage(page)

    await expect(page.getByText('svc-alpha.service').first()).toBeVisible()
    await expect(page.getByText('svc-beta.service').first()).toBeVisible()

    await expect(page.getByText('有新版本')).toBeVisible()
    await expect(page.getByText('有更高版本')).toBeVisible()
    await expect(page.getByText('latest')).toBeVisible()

    await page.getByLabel('Dry run').check()

    await page.getByPlaceholder('who is triggering').fill('ui-e2e')
    await page.getByPlaceholder('short free-form reason').fill('deploy-all dry-run')

    await page.getByRole('button', { name: '部署全部服务' }).click()

    await expect(page.getByText('部署请求已提交')).toBeVisible()

    await expect(page.getByText(/deploy-all \(/)).toBeVisible()
  })

  test('supports per-service deploy with dry toggle', async ({ page }) => {
    await openManualPage(page)

    const row = page.locator('form', { hasText: 'svc-alpha.service' }).first()

    await row.getByPlaceholder(/image/).fill('ghcr.io/example/image:ui-e2e')
    await row.getByPlaceholder('caller').fill('ui-e2e')
    await row.getByPlaceholder('reason').fill('single-unit test')

    await row.getByLabel('Dry').check()

    await row.getByRole('button', { name: '升级' }).click()

    await expect(page.getByText('服务升级已提交')).toBeVisible()
    await expect(page.getByText(/upgrade-service svc-alpha\.service/)).toBeVisible()
  })

  test('shows error toast when deploy-all fails', async ({ page }) => {
    await page.addInitScript(() => {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      ;(window as any).__MOCK_FORCE_MANUAL_FAILURE__ = true
    })

    await page.route('**/api/manual/deploy', async (route) => {
      await route.fulfill({
        status: 500,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'simulated failure' }),
      })
    })

    await openManualPage(page)

    await page.getByRole('button', { name: '部署全部服务' }).click()

    await expect(page.getByText('部署失败')).toBeVisible()
    await expect(page.getByText('暂无手动操作记录。')).toBeVisible()
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
    await page.getByRole('button', { name: '部署全部服务' }).click()

    await expect(page.getByText('部署请求已提交')).toBeVisible()

    const historyEntry = page.getByRole('button', {
      name: /deploy-all \(/,
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

  test('shows meta.result_message details in Manual task drawer timeline (collapsible)', async ({ page }) => {
    await openManualPage(page)

    const svcRow = page.locator('form', { hasText: 'svc-alpha.service' }).first()
    await svcRow.getByLabel('Dry').uncheck()
    await svcRow.getByRole('button', { name: '升级' }).click()

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

  test.describe('Tasks drawer URL deep links', () => {
    const installDeterministicMockSeed = async (page: import('@playwright/test').Page) => {
      await page.addInitScript(({ nowMs, seed }) => {
        // Make mock runtime data deterministic across reloads so deeplink behavior is stable.
        Date.now = () => nowMs

        const mulberry32 = (a) => {
          return () => {
            let t = (a += 0x6d2b79f5)
            t = Math.imul(t ^ (t >>> 15), t | 1)
            t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
            return ((t ^ (t >>> 14)) >>> 0) / 4294967296
          }
        }

        Math.random = mulberry32(seed)
      }, { nowMs: 1_731_000_000_000, seed: 1337 })
    }

    const getDrawerTaskId = async (page: import('@playwright/test').Page) => {
      await expect(page).toHaveURL(/task_id=tsk_/)
      const url = new URL(page.url())
      const id = url.searchParams.get('task_id')
      expect(id).toMatch(/^tsk_/)
      return id ?? ''
    }

    test('opens list deeplink, closes via overlay, and preserves mock params', async ({ page }) => {
      await installDeterministicMockSeed(page)

      await page.goto('/services?drawer=tasks&mock=enabled&mock=profile=happy-path')
      await page.waitForFunction(() => (window as any).__MOCK_ENABLED__ === true)

      await expect(page.getByText('任务中心')).toBeVisible()
      await expect(page.getByRole('button', { name: '任务列表' })).toBeVisible()
      await expect(page.locator('table tbody tr').first()).toBeVisible()

      // Restore from URL should be stable across reloads.
      await page.reload()
      await page.waitForFunction(() => (window as any).__MOCK_ENABLED__ === true)
      await expect(page.getByText('任务中心')).toBeVisible()
      await expect(page.getByRole('button', { name: '任务列表' })).toBeVisible()
      await expect(page.locator('table tbody tr').first()).toBeVisible()

      // Overlay click closes drawer.
      const overlay = page.getByRole('button', { name: '关闭任务中心' })
      await overlay.click({ position: { x: 1, y: 1 } })

      await expect(page.getByText('任务中心')).toHaveCount(0)

      const url = new URL(page.url())
      expect(url.pathname).toBe('/services')
      expect(url.searchParams.getAll('mock')).toEqual(['enabled', 'profile=happy-path'])
      expect(url.searchParams.get('drawer')).toBeNull()
      expect(url.searchParams.get('task_id')).toBeNull()
    })

    test('opens detail deeplink, shows task_id, and list/detail switches update URL', async ({ page }) => {
      await installDeterministicMockSeed(page)

      // Discover a stable task id in this seeded mock session.
      await page.goto('/services?drawer=tasks&mock=enabled&mock=profile=happy-path')
      await page.waitForFunction(() => (window as any).__MOCK_ENABLED__ === true)

      await expect(page.getByText('任务中心')).toBeVisible()
      await expect(page.getByRole('button', { name: '任务列表' })).toBeVisible()
      const firstRow = page.locator('table tbody tr').filter({ hasText: 'Manual' }).first()
      await expect(firstRow).toBeVisible()
      await firstRow.click()
      await expect(page.getByRole('button', { name: '任务详情' })).toBeVisible()

      const taskId = await getDrawerTaskId(page)

      // Open the detail deeplink directly using the known task id.
      await page.goto(`/services?drawer=tasks&task_id=${taskId}&mock=enabled&mock=profile=happy-path`)
      await page.waitForFunction(() => (window as any).__MOCK_ENABLED__ === true)

      await expect(page.getByText('任务中心')).toBeVisible()
      await expect(page.getByRole('button', { name: '任务详情' })).toBeVisible()
      await expect(page.getByText(taskId)).toBeVisible()

      // Restore from URL should be stable across reloads.
      await page.reload()
      await page.waitForFunction(() => (window as any).__MOCK_ENABLED__ === true)
      await expect(page.getByText('任务中心')).toBeVisible()
      await expect(page.getByText(taskId)).toBeVisible()

      // Switch to list removes task_id but keeps drawer=tasks.
      await page.getByRole('button', { name: '任务列表' }).click()
      await expect(page).toHaveURL(/\/services\?/)
      expect(page.url()).toContain('drawer=tasks')
      expect(page.url()).not.toContain('task_id=')

      // Clicking a row opens detail and sets task_id.
      const row = page.locator('table tbody tr').filter({ hasText: 'Manual' }).first()
      await expect(row).toBeVisible()
      await row.click()
      const taskId2 = await getDrawerTaskId(page)
      expect(page.url()).toContain(`task_id=${taskId2}`)

      // "Open in Tasks" navigates to /tasks?task_id=...
      await page.getByRole('button', { name: '在 Tasks 中打开' }).click()
      await expect(page).toHaveURL(`/tasks?task_id=${taskId2}`)
    })
  })
})
