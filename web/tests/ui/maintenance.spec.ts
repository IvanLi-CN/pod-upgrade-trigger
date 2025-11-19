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
    await expect(payloadRow).toBeVisible()
  })

  test('can trigger prune-state cleanup', async ({ page }) => {
    await page.goto('/maintenance')

    await page.getByLabel('最大保留时间（小时）').fill('24')
    await page.getByRole('button', { name: '清理' }).click()

    await expect(page.getByText('清理完成')).toBeVisible()
  })

  test('shows and downloads debug payload after signature mismatch', async ({
    page,
    request,
  }) => {
    const body = {
      package: {
        package_type: 'container',
        name: 'svc-alpha',
        owner: { login: 'example' },
      },
      registry: {
        host: 'ghcr.io',
      },
      package_version: {
        metadata: {
          container: {
            tags: ['latest'],
          },
        },
      },
    }

    await request.post('/github-package-update/svc-alpha', {
      headers: {
        'content-type': 'application/json',
        'x-hub-signature-256': 'sha256=00',
        'x-github-event': 'package',
      },
      data: body,
    })

    await page.goto('/maintenance')

    const payloadRow = page.getByRole('row', { name: /last_payload\.bin/ })
    const payloadStatusCell = payloadRow.getByRole('cell').nth(1)
    await expect(payloadStatusCell.getByText('存在')).toBeVisible()

    const downloadResponse = await request.get('/last_payload.bin')
    await expect(downloadResponse.status()).toBe(200)
    const downloaded = await downloadResponse.body()
    expect(downloaded.byteLength).toBeGreaterThan(0)
  })
})
