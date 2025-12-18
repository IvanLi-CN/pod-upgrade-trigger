import { expect, test } from '@playwright/test'

test.describe('Dashboard and navigation', () => {
  test('loads dashboard and basic navigation', async ({ page }) => {
    await page.goto('/')

    await expect(page.getByText('Pod Upgrade Trigger')).toBeVisible()

    await expect(page.getByText('Service healthy')).toBeVisible({ timeout: 10_000 })

    await expect(page.getByText('SSE ok')).toBeVisible({ timeout: 10_000 })

    const nav = page.getByRole('navigation')
    await expect(nav.getByRole('link', { name: 'Dashboard' })).toBeVisible()
    await expect(nav.getByRole('link', { name: 'Services' })).toBeVisible()
    await expect(nav.getByRole('link', { name: 'Webhooks' })).toBeVisible()
    await expect(nav.getByRole('link', { name: 'Events' })).toBeVisible()
    await expect(nav.getByRole('link', { name: 'Maintenance' })).toBeVisible()
    await expect(nav.getByRole('link', { name: 'Settings' })).toBeVisible()

    await page.getByRole('link', { name: 'Services' }).click()
    await expect(page).toHaveURL(/\/services$/)

    await page.getByRole('link', { name: 'Webhooks' }).click()
    await expect(page).toHaveURL(/\/webhooks$/)

    await page.getByRole('link', { name: 'Events' }).click()
    await expect(page).toHaveURL(/\/events$/)

    await page.getByRole('link', { name: 'Maintenance' }).click()
    await expect(page).toHaveURL(/\/maintenance$/)

    await page.getByRole('link', { name: 'Settings' }).click()
    await expect(page).toHaveURL(/\/settings$/)
  })

  test('supports direct deep links for core routes', async ({ page }) => {
    const paths = ['/services', '/manual', '/webhooks', '/events', '/maintenance', '/settings']

    const gotoStable = async (path: string) => {
      // React router can trigger immediate client-side redirects (e.g. /manual -> /services),
      // which may occasionally interrupt the navigation Playwright is waiting on.
      for (let attempt = 0; attempt < 2; attempt += 1) {
        try {
          await page.goto(path)
          return
        } catch (error) {
          const message = String(error)
          if (message.includes('interrupted by another navigation')) {
            continue
          }
          throw error
        }
      }
      await page.goto(path)
    }

    for (const path of paths) {
      await gotoStable(path)
      if (path === '/manual') {
        await expect(page).toHaveURL(/\/services(\?|#|$)/)
      }
      await expect(page.getByText('404 · 页面不存在')).toHaveCount(0)
    }
  })
})
