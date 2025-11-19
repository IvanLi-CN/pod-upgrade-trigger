import { expect, test } from '@playwright/test'

test.describe('Webhooks page', () => {
  test('shows loading state then unit cards', async ({ page }) => {
    await page.route('**/api/webhooks/status', async (route) => {
      await new Promise((resolve) => setTimeout(resolve, 200))
      await route.continue()
    })

    await page.route('**/api/config', async (route) => {
      await new Promise((resolve) => setTimeout(resolve, 200))
      await route.continue()
    })

    await page.goto('/webhooks')

    const loading = page.getByText('Loading config and webhook status…')
    await expect(loading).toBeVisible()
    await expect(loading).not.toBeVisible()

    await expect(page.getByText('svc-alpha.service').first()).toBeVisible()
    await expect(page.getByText('svc-beta.service').first()).toBeVisible()

    const copyButton = page.getByRole('button', { name: '复制 URL' }).first()
    await expect(copyButton).toBeEnabled()
  })

  test('builds full webhook URL and opens events view', async ({ page }) => {
    await page.goto('/webhooks')

    const alphaCard = page
      .locator('div.rounded-lg.border')
      .filter({ hasText: 'svc-alpha.service' })
      .first()
    await expect(alphaCard).toBeVisible()

    const webhookBadge = alphaCard
      .locator('.badge')
      .filter({ hasText: '/github-package-update/svc-alpha' })
      .first()

    await expect(webhookBadge).toContainText(
      'http://127.0.0.1:25211/github-package-update/svc-alpha',
    )

    await page.getByRole('button', { name: '查看事件' }).first().click()

    await expect(page).toHaveURL(/\/events\?path_prefix=/)

    const pathPrefixInput = page.getByLabel('Path prefix')
    await expect(pathPrefixInput).toHaveValue(/github-package-update\//)
  })
})
