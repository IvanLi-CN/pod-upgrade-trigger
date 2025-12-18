import { expect, test } from '@playwright/test'

async function gotoProfile(
  page: Parameters<typeof test>[0]['page'],
  path: string,
  profile: string,
) {
  const url = `${path}?mock=enabled&mock=profile=${profile}`
  await page.goto(url)
  await page.waitForFunction(() => (window as any).__MOCK_ENABLED__ === true)
}

async function gotoWithMock(
  page: Parameters<typeof test>[0]['page'],
  path: string,
) {
  const url = path.includes('?') ? `${path}&mock=enabled` : `${path}?mock=enabled`
  await page.goto(url)
  await page.waitForFunction(() => (window as any).__MOCK_ENABLED__ === true)
}

async function resetProfile(page: Parameters<typeof test>[0]['page']) {
  await page.evaluate(() => {
    localStorage.removeItem('mock:profile')
  })
}

test.describe('Mock profile: empty-state', () => {
  test('shows empty manual and tasks views without errors', async ({ page }) => {
    await gotoProfile(page, '/services', 'empty-state')

    await expect(page.getByText('按服务部署')).toBeVisible()
    await expect(
      page.getByText('暂无可部署的服务。'),
    ).toBeVisible()
    await expect(
      page.getByText('暂无手动部署记录。'),
    ).toBeVisible()

    await gotoProfile(page, '/tasks', 'empty-state')

    await expect(page.getByText('任务列表')).toBeVisible()
    await expect(
      page.getByText('当前没有符合条件的任务记录。'),
    ).toBeVisible()

    await resetProfile(page)
  })
})

test.describe('Mock profile: rate-limit-hot', () => {
  test('increases events volume and supports filters', async ({ page }) => {
    await gotoProfile(page, '/events', 'happy-path')

    await expect(page.getByText('事件与审计')).toBeVisible()
    const baseRows = page.locator('table tbody tr')
    await expect(baseRows.first()).toBeVisible()
    const baseCount = await baseRows.count()

    await gotoProfile(page, '/events', 'rate-limit-hot')

    await expect(page.getByText('事件与审计')).toBeVisible()
    const hotRows = page.locator('table tbody tr')
    await expect(hotRows.first()).toBeVisible()
    const hotCount = await hotRows.count()
    expect(hotCount).toBeGreaterThan(baseCount)

    // Path prefix filter
    await page.getByLabel('Path prefix').fill('/api/manual/services/svc-beta')
    const prefixRows = page.locator('table tbody tr')
    await expect(prefixRows.first()).toBeVisible()
    await expect(
      prefixRows.first().locator('td').nth(3),
    ).toContainText('svc-beta')

    // Status filter
    await page.getByLabel('Path prefix').fill('')
    await page.getByLabel('Status').fill('429')
    const statusRows = page.locator('table tbody tr')
    await expect(statusRows.first()).toBeVisible()
    await expect(
      statusRows.first().locator('td').nth(4),
    ).toContainText('429')

    // Action filter
    await page.getByLabel('Status').fill('')
    await page.getByLabel('Action').fill('manual-trigger')
    const actionRows = page.locator('table tbody tr')
    await expect(actionRows.first()).toBeVisible()
    await expect(
      actionRows.first().locator('td').nth(5),
    ).toHaveText('manual-trigger')

    await resetProfile(page)
  })

  test('shows HMAC error and extra image locks for svc-beta', async ({ page }) => {
    await gotoProfile(page, '/webhooks', 'happy-path')

    await expect(
      page.getByRole('heading', { name: 'GitHub Webhooks' }),
    ).toBeVisible()

    const locksSection = page
      .locator('section')
      .filter({ hasText: '镜像锁' })
      .first()
    const baseLockRows = locksSection.locator('tbody tr')
    await expect(baseLockRows.first()).toBeVisible()
    const baseLockCount = await baseLockRows.count()

    await gotoProfile(page, '/webhooks', 'rate-limit-hot')

    await expect(
      page.getByRole('heading', { name: 'GitHub Webhooks' }),
    ).toBeVisible()

    const betaCard = page
      .locator('div.rounded-lg.border')
      .filter({ hasText: 'svc-beta.service' })
      .first()

    await expect(betaCard.getByText('HMAC Error')).toBeVisible()
    await expect(
      betaCard.getByText('hmac · signature mismatch'),
    ).toBeVisible()

    const hotLocksSection = page
      .locator('section')
      .filter({ hasText: '镜像锁' })
      .first()
    const hotLockRows = hotLocksSection.locator('tbody tr')
    await expect(hotLockRows.first()).toBeVisible()
    const hotLockCount = await hotLockRows.count()
    expect(hotLockCount).toBeGreaterThan(baseLockCount)

    await resetProfile(page)
  })
})

test.describe('Mock profile: degraded', () => {
  test('propagates degraded health and SSE error from mocks to UI', async ({ page }) => {
    await gotoProfile(page, '/', 'degraded')

    await expect(page.getByText('Pod Upgrade Trigger')).toBeVisible()

    await expect(
      page.getByRole('banner').getByText('Degraded'),
    ).toBeVisible({ timeout: 10_000 })
    await expect(page.getByText('SSE error')).toBeVisible({ timeout: 10_000 })

    const apiStatuses = await page.evaluate(async () => {
      const healthRes = await fetch('/health', { cache: 'no-store' })
      const sseRes = await fetch('/sse/hello', { cache: 'no-store' })
      const healthBody = await healthRes.text()
      const sseBody = await sseRes.text()
      return {
        healthStatus: healthRes.status,
        healthBody,
        sseStatus: sseRes.status,
        sseBody,
      }
    })

    expect(apiStatuses.healthStatus).toBe(503)
    expect(apiStatuses.healthBody).toContain('fail')
    expect(apiStatuses.sseStatus).toBe(503)
    expect(apiStatuses.sseBody).toContain('sse down')

    await resetProfile(page)
  })
})

test.describe('Mock console profile switching', () => {
  test('switches profiles and persists selection across reloads', async ({ page }) => {
    await gotoWithMock(page, '/services')

    await expect(
      page.getByText('暂无可部署的服务。'),
    ).toHaveCount(0)

    const consoleToggle = page.getByRole('button', { name: 'Mock 控制台' })
    await expect(consoleToggle).toBeVisible()
    await consoleToggle.click()

    const emptyButton = page.getByRole('button', { name: 'empty-state' })
    await expect(emptyButton).toBeVisible()
    await emptyButton.click()

    const storedProfile = await page.evaluate(() =>
      localStorage.getItem('mock:profile'),
    )
    expect(storedProfile).toBe('empty-state')

    await page.reload()

    await expect(
      page.getByText('暂无可部署的服务。'),
    ).toBeVisible()

    await resetProfile(page)
  })
})
