import { expect, test } from '@playwright/test'

async function openManualPage(page: Parameters<typeof test>[0]['page']) {
  await page.goto('/manual')
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
})
