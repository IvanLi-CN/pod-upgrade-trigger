import { expect, test } from '@playwright/test'

test.describe('Settings page', () => {
  test('shows environment and systemd configuration', async ({ page }) => {
    await page.goto('/settings')

    const stateRow = page.getByRole('row', { name: /PODUP_STATE_DIR/ })
    await expect(stateRow.getByText('configured')).toBeVisible()
    await expect(stateRow.getByRole('cell').nth(2)).not.toHaveText('(empty)')

    const tokenRow = page.getByRole('row', { name: /PODUP_TOKEN/ })
    await expect(tokenRow.getByText('configured')).toBeVisible()
    await expect(tokenRow.getByRole('cell').nth(2)).toHaveText('***')

    const webhookSecretRow = page.getByRole('row', { name: /PODUP_GH_WEBHOOK_SECRET/ })
    await expect(webhookSecretRow.getByText('configured')).toBeVisible()
    await expect(webhookSecretRow.getByRole('cell').nth(2)).toHaveText('***')

    const forwardCard = page.locator('section').filter({ hasText: 'ForwardAuth' }).first()

    const headerLine = forwardCard.locator('li').filter({ hasText: 'Header:' }).first()
    await expect(headerLine.locator('code')).toHaveText('(not configured)')

    const adminConfiguredLine = forwardCard
      .locator('li')
      .filter({ hasText: 'Admin value configured:' })
      .first()
    await expect(adminConfiguredLine.locator('code')).toHaveText('no')

    const devOpenLine = forwardCard
      .locator('li')
      .filter({ hasText: 'DEV_OPEN_ADMIN:' })
      .first()
    await expect(devOpenLine.locator('code')).toHaveText('true')

    const modeLine = forwardCard.locator('li').filter({ hasText: 'Mode:' }).first()
    await expect(modeLine.locator('code')).toHaveText('open')

    const systemdCard = page.locator('section').filter({ hasText: 'systemd 单元' }).first()
    await expect(systemdCard).toBeVisible()
    await expect(
      systemdCard.getByText('auto-update unit 以及可通过 /api/manual 触发的业务单元。'),
    ).toBeVisible()

    const manualLink = systemdCard.getByRole('link', { name: '手动触发' }).first()
    await manualLink.click()
    await expect(page).toHaveURL(/\/manual$/)
  })
})
