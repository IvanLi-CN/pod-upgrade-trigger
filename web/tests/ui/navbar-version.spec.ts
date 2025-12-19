import { expect, test } from '@playwright/test'

test.describe('Navbar version + self-update (mock)', () => {
  test('shows current version, shows update dropdown, triggers self-update and navigates to /tasks', async ({
    page,
  }) => {
    await page.addInitScript(() => {
      localStorage.removeItem('podup_version_last_check')
      localStorage.removeItem('podup_version_latest_tag')
    })

    const versionCheckResponse = page.waitForResponse(
      (res) => res.url().includes('/api/version/check') && res.status() === 200,
    )

    await page.goto('/?mock=enabled')
    await page.waitForFunction(() => (window as any).__MOCK_ENABLED__ === true)
    await versionCheckResponse

    const banner = page.getByRole('banner')

    await expect(
      banner.getByText('Pod Upgrade Trigger', { exact: true }),
    ).toBeVisible()

    // Current version badge is always visible (mock settings.package=0.9.1).
    await expect(banner.getByText('v0.9.1', { exact: true })).toBeVisible()

    // Update dropdown trigger (mock latest.release_tag=v0.9.2).
    await expect(banner.getByText('v0.9.2', { exact: true })).toBeVisible()
    const updateTrigger = banner.getByRole('button', {
      name: '新版本菜单 v0.9.2',
    })
    await expect(updateTrigger).toBeVisible()
    await updateTrigger.click()

    const updateNow = page.getByRole('button', { name: '立即更新' })
    await expect(updateNow).toBeVisible()

    const codeLink = page.getByRole('link', { name: '跳转到该版本代码页' })
    await expect(codeLink).toBeVisible()
    await expect(codeLink).toHaveAttribute(
      'href',
      'https://github.com/ivanli-cn/pod-upgrade-trigger/tree/v0.9.2',
    )

    await updateNow.click()

    const confirmDialog = page.getByRole('dialog', { name: '自更新确认对话框' })
    await expect(confirmDialog).toBeVisible()
    await expect(confirmDialog.getByText('确认立即更新？')).toBeVisible()

    await confirmDialog.getByRole('button', { name: '确认更新' }).click()

    await expect(page).toHaveURL(/\/tasks\?task_id=/)
    await expect(page.getByText('任务详情', { exact: true })).toBeVisible()
  })
})
