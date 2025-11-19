import { expect, test } from '@playwright/test'

const AUTH_BASE =
  process.env.UI_E2E_AUTH_BASE_URL ?? 'http://127.0.0.1:25212'

test.describe('401 Unauthorized page', () => {
  test('renders 401 view and preserves original path', async ({ page }) => {
    await page.goto(`${AUTH_BASE}/settings`)

    await expect(page.getByText('未授权 · 401')).toBeVisible()

    await expect(page.getByText('当前请求路径：')).toContainText('/settings')

    await expect(page).toHaveURL(/\/settings$/)

    await page.getByRole('button', { name: '刷新重试' }).click()

    await expect(page).toHaveURL(/\/settings$/)
  })
})

