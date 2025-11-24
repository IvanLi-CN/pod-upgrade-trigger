import { expect, test } from '@playwright/test'

const AUTH_BASE =
  process.env.UI_E2E_AUTH_BASE_URL ??
  process.env.UI_E2E_BASE_URL ??
  'http://127.0.0.1:25211'

test.describe('401 Unauthorized page', () => {
  test('renders 401 view and preserves original path', async ({ page }) => {
    const url = new URL(AUTH_BASE)
    url.pathname = '/settings'
    url.searchParams.append('mock', 'profile=auth-error')
    if (!url.searchParams.has('mock')) {
      url.searchParams.append('mock', 'enabled')
    }

    await page.goto(url.toString())

    await page.waitForFunction(() => window.__MOCK_ENABLED__ === true)

    const apiStatus = await page.evaluate(async () => {
      const res = await fetch('/api/settings', { cache: 'no-store' })
      return res.status
    })

    expect(apiStatus).toBe(401)

    await expect(page.getByText('未授权 · 401')).toBeVisible()

    await expect(page.getByText('当前请求路径：')).toContainText('/settings')

    await expect(page).toHaveURL(/\/401(\?.*)?$/)

    await page.getByRole('button', { name: '刷新重试' }).click()

    await expect(page).toHaveURL(/\/401(\?.*)?$/)

    // ensure later tests start with default mock profile
    await page.evaluate(() => {
      localStorage.removeItem('mock:profile')
    })
  })
})
