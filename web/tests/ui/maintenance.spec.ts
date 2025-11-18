import { expect, test } from '@playwright/test'

test.describe('Maintenance page', () => {
  test('shows state directory resources', async ({ page }) => {
    await page.goto('/maintenance')

    await expect(page.getByRole('heading', { name: '状态目录检查' })).toBeVisible()

    const dbRow = page.getByRole('row', { name: /pod-upgrade-trigger\.db/ })
    await expect(dbRow.getByText('存在')).toBeVisible()

    const webDistRow = page.getByRole('row', { name: /web\/dist/ })
    await expect(webDistRow.getByText('存在')).toBeVisible()

    const payloadRow = page.getByRole('row', { name: /last_payload\.bin/ })
    const payloadStatusCell = payloadRow.getByRole('cell').nth(1)
    await expect(payloadStatusCell.getByText('缺失', { exact: true })).toBeVisible()
  })
})
