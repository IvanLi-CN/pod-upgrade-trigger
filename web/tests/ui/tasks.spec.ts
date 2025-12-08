import fs from 'node:fs'
import { expect, test } from '@playwright/test'

async function openTasksPage(page: Parameters<typeof test>[0]['page']) {
  await page.goto('/tasks?mock=enabled')
  await page.waitForFunction(() => (window as any).__MOCK_ENABLED__ === true)
  await expect(page.getByText('任务列表')).toBeVisible()

  const rows = page.locator('table tbody tr')
  await expect(rows.first()).toBeVisible()
  return rows
}

async function resetMockData(page: Parameters<typeof test>[0]['page']) {
  const consoleToggle = page.getByRole('button', { name: 'Mock 控制台' })
  await expect(consoleToggle).toBeVisible()
  await consoleToggle.click()
  const resetButton = page.getByRole('button', { name: '重置数据' })
  await expect(resetButton).toBeVisible()
  await resetButton.click()
}

test.describe('Tasks page (mock)', () => {
  test('lists tasks and shows details drawer with units and timeline', async ({ page }) => {
    const rows = await openTasksPage(page)

    const manualRow = rows
      .filter({ hasText: 'nightly manual upgrade' })
      .first()
    await expect(manualRow).toBeVisible()

    const cells = manualRow.locator('td')

    await expect(cells.nth(0)).not.toBeEmpty() // 类型
    await expect(cells.nth(1)).not.toBeEmpty() // 状态
    await expect(cells.nth(2)).toContainText('units') // unit 汇总
    await expect(cells.nth(3)).not.toBeEmpty() // 触发来源
    await expect(cells.nth(4)).not.toBeEmpty() // 开始时间
    await expect(cells.nth(5)).not.toBeEmpty() // 耗时
    await expect(cells.nth(6)).not.toBeEmpty() // 摘要

    await manualRow.click()

    await expect(page.getByText('任务详情', { exact: true })).toBeVisible()
    await expect(page.getByText('nightly manual upgrade')).toBeVisible()

    await expect(page.getByText('创建 ·')).toBeVisible()
    await expect(page.getByText('起止 ·')).toBeVisible()
    await expect(page.getByText('耗时 ·')).toBeVisible()

    await expect(page.getByText('来源 · manual')).toBeVisible()
    await expect(page.getByText('caller · ops-nightly')).toBeVisible()
    await expect(page.getByText('reason · nightly rollout')).toBeVisible()
    await expect(page.getByText('path · /api/manual/trigger')).toBeVisible()

    await expect(page.getByText('单元状态')).toBeVisible()
    await expect(
      page.getByText('svc-alpha.service', { exact: true }).first(),
    ).toBeVisible()
    await expect(
      page.getByText('svc-beta.service', { exact: true }).first(),
    ).toBeVisible()

    await expect(page.getByText('pulled image and restarted successfully')).toBeVisible()
    await expect(page.getByText('restart completed')).toBeVisible()

    await expect(page.getByText('日志时间线')).toBeVisible()
    await expect(page.getByText('task-created')).toBeVisible()
    await expect(page.getByText('image-pull')).toBeVisible()
    await expect(
      page.getByText('Restarted svc-alpha.service, svc-beta.service'),
    ).toBeVisible()
  })

  test('shows command output for command-meta logs in the timeline', async ({ page }) => {
    // 对于命令输出用例，我们需要在 mock happy-path 的初始快照上，
    // 并且保证任务列表与 runtime 内部数据一致。
    await page.goto('/tasks?mock=enabled&mock=profile=happy-path')
    await page.waitForFunction(() => (window as any).__MOCK_ENABLED__ === true)

    // 先重置 mock 数据，再刷新页面让 /api/tasks 与 runtime 同步。
    await resetMockData(page)

    await page.reload()
    await page.waitForFunction(() => (window as any).__MOCK_ENABLED__ === true)
    await expect(page.getByText('任务列表')).toBeVisible()

    const rows = page.locator('table tbody tr')
    await expect(rows.first()).toBeVisible()

    const manualRow = rows
      .filter({ hasText: 'nightly manual upgrade' })
      .first()
    await expect(manualRow).toBeVisible()

    await manualRow.click()

    const logsTimelineSection = page
      .locator('section')
      .filter({ hasText: '日志时间线' })
      .first()
    await expect(logsTimelineSection).toBeVisible()

    const imagePullLabel = logsTimelineSection.getByText('image-pull').first()
    await imagePullLabel.scrollIntoViewIfNeeded()
    await expect(imagePullLabel).toBeVisible()

    const commandToggle = logsTimelineSection
      .getByRole('button', { name: '命令输出' })
      .first()
    await commandToggle.scrollIntoViewIfNeeded()
    await expect(
      commandToggle,
      'command output toggle not visible in image-pull log card',
    ).toBeVisible()

    await commandToggle.click()

    await expect(
      logsTimelineSection.getByText(
        'podman pull ghcr.io/example/svc-alpha:main',
      ),
    ).toBeVisible()
    await expect(
      logsTimelineSection.getByText('pulling from registry.example...'),
    ).toBeVisible()
    await expect(
      logsTimelineSection.getByText(
        'warning: using cached image layer metadata',
      ),
    ).toBeVisible()
  })

  test('supports stopping a running task and updates status and logs', async ({ page }) => {
    const rows = await openTasksPage(page)

    const row = rows
      .filter({ hasText: 'Auto-update in progress for podman-auto-update.service' })
      .first()
    await expect(row).toBeVisible()

    const statusCell = row.locator('td').nth(1)
    await expect(statusCell).toHaveText('running')

    await row.click()

    const stopButton = page.getByRole('button', { name: '停止', exact: true })
    await expect(stopButton).toBeEnabled()
    await stopButton.click()

    await expect(page.getByText('任务已请求停止')).toBeVisible()

    await expect(statusCell).toHaveText('cancelled')
    await expect(
      page.getByText('Task cancelled via mock /stop endpoint'),
    ).toBeVisible()

    await resetMockData(page)
  })

  test('shows scheduler-triggered tasks with correct source, summary and timeline', async ({
    page,
  }) => {
    const rows = await openTasksPage(page)

    const schedulerRow = rows
      .filter({
        hasText: 'Auto-update in progress for podman-auto-update.service',
      })
      .first()
    await expect(schedulerRow).toBeVisible()

    const cells = schedulerRow.locator('td')

    await expect(cells.nth(0).getByText('Scheduler')).toBeVisible()
    await expect(cells.nth(1).getByText('running')).toBeVisible()
    await expect(cells.nth(3)).toContainText('scheduler')
    await expect(
      cells.nth(6).getByText('Auto-update in progress for podman-auto-update.service'),
    ).toBeVisible()

    await schedulerRow.click()

    await expect(page.getByText('任务详情', { exact: true })).toBeVisible()
    await expect(page.getByText('Auto-update in progress for podman-auto-update.service')).toBeVisible()

    await expect(page.getByText('来源 · scheduler')).toBeVisible()

    const unitsSection = page
      .locator('section')
      .filter({ hasText: '单元状态' })
      .first()
    await expect(
      unitsSection.getByText('podman-auto-update.service', { exact: true }),
    ).toBeVisible()
    await expect(
      unitsSection.getByText('Checking images and applying updates'),
    ).toBeVisible()

    const timeline = page
      .locator('section')
      .filter({ hasText: '日志时间线' })
      .first()
    await expect(timeline).toBeVisible()

    await expect(timeline.getByText('Scheduler iteration #84 started')).toBeVisible()
    await expect(timeline.getByText('Scanning auto-update units')).toBeVisible()
  })

  test('supports force-stopping a running task', async ({ page }) => {
    const rows = await openTasksPage(page)

    const row = rows
      .filter({ hasText: 'Auto-update in progress for podman-auto-update.service' })
      .first()
    await expect(row).toBeVisible()

    const statusCell = row.locator('td').nth(1)
    await expect(statusCell).toHaveText('running')

    await row.click()

    const forceButton = page.getByRole('button', { name: '强制停止' })
    await expect(forceButton).toBeEnabled()
    await forceButton.click()

    await expect(page.getByText('已强制停止任务')).toBeVisible()

    await expect(statusCell).toHaveText('failed')
    await expect(
      page.getByText('Task force-stopped via mock /force-stop endpoint'),
    ).toBeVisible()

    await resetMockData(page)
  })

  test('supports retrying a finished task and focuses the new retry task', async ({ page }) => {
    await openTasksPage(page)

    await page.getByLabel('状态').selectOption('succeeded')
    await page.getByLabel('类型').selectOption('manual')

    const filteredRows = page.locator('table tbody tr')
    const row = filteredRows
      .filter({ hasText: 'nightly manual upgrade' })
      .first()
    await expect(row).toBeVisible()
    await row.click()

    await expect(page.getByText('任务详情', { exact: true })).toBeVisible()

    const idBadge = page.locator('.badge.font-mono').first()
    await expect(idBadge).toBeVisible()
    const originalTaskId = (await idBadge.textContent())?.trim() ?? ''
    expect(originalTaskId).not.toEqual('')

    const retryButton = page.getByRole('button', { name: '重试' })
    await expect(retryButton).toBeEnabled()
    await retryButton.click()

    await expect(page.getByText('已创建重试任务')).toBeVisible()

    const newTaskId = (await idBadge.textContent())?.trim() ?? ''
    expect(newTaskId).not.toEqual('')
    expect(newTaskId).not.toEqual(originalTaskId)
    expect(newTaskId).toMatch(/^retry_/)

    const firstRow = page.locator('table tbody tr').first()
    await expect(firstRow.locator('td').nth(1)).toHaveText('pending')
    await expect(firstRow.locator('td').nth(6)).toContainText('retry')
  })

  test('opens related events from task drawer', async ({ page }) => {
    await page.goto('/tasks?mock=enabled&mock=profile=happy-path')
    await page.waitForFunction(() => (window as any).__MOCK_ENABLED__ === true)

    await expect(page.getByText('任务中心')).toBeVisible()

    const rows = page.locator('table tbody tr')
    await expect(rows.first()).toBeVisible()

    const manualRow = rows.filter({ hasText: 'nightly manual upgrade' }).first()
    await expect(manualRow).toBeVisible()

    await manualRow.click()

    const link = page.getByRole('link', { name: '查看关联事件' })
    await expect(link).toBeVisible()
    await link.click()

    await expect(page).toHaveURL(/\/events\?/)
    const url = page.url()
    expect(url).toContain('task_id=')

    await expect(page.getByText('事件与审计')).toBeVisible()

    const eventRows = page.locator('table tbody tr')
    await expect(eventRows.first()).toBeVisible()
  })

  test('exports task detail as JSON', async ({ page }) => {
    const rows = await openTasksPage(page)

    const manualRow = rows
      .filter({ hasText: 'nightly manual upgrade' })
      .first()
    await expect(manualRow).toBeVisible()

    await manualRow.click()

    const exportButton = page.getByRole('button', { name: '导出 JSON' })
    await expect(exportButton).toBeVisible()

    const [download] = await Promise.all([
      page.waitForEvent('download'),
      exportButton.click(),
    ])

    const downloadPath = await download.path()
    if (!downloadPath) {
      throw new Error('Download path is not available')
    }

    const content = fs.readFileSync(downloadPath, 'utf-8')
    expect(content).toContain('"task_id"')
    expect(content).toContain('"logs"')
  })

  test('renders unknown task status and auto-update run without JSONL summary', async ({ page }) => {
    const rows = await openTasksPage(page)

    const unknownRow = rows
      .filter({
        hasText:
          'podman auto-update run completed (no JSONL summary found',
      })
      .first()
    await expect(unknownRow).toBeVisible()

    const statusCell = unknownRow.locator('td').nth(1)
    await expect(statusCell.getByText('Unknown')).toBeVisible()

    await unknownRow.click()

    await expect(page.getByText('任务详情', { exact: true })).toBeVisible()
    await expect(page.getByText('Status unknown')).toBeVisible()

    const timeline = page
      .locator('section')
      .filter({ hasText: '日志时间线' })
      .first()
    await expect(timeline).toBeVisible()

    await expect(timeline.getByText('auto-update-run')).toBeVisible()
    await expect(
      timeline.getByText('no JSONL summary found', { exact: true }),
    ).toBeVisible()
  })

  test('highlights image-prune logs with best-effort semantics and command meta', async ({ page }) => {
    const rows = await openTasksPage(page)

    const pruneRow = rows
      .filter({ hasText: 'webhook with image prune' })
      .first()
    await expect(pruneRow).toBeVisible()

    await pruneRow.click()

    const timeline = page
      .locator('section')
      .filter({ hasText: '日志时间线' })
      .first()
    await expect(timeline).toBeVisible()

    const pruneFailureCard = timeline
      .locator(
        'div:has(> p:has-text("Image prune failed (best-effort clean-up)"))',
      )
      .first()
    await pruneFailureCard.scrollIntoViewIfNeeded()
    await expect(pruneFailureCard).toBeVisible()
    await expect(
      timeline.getByText('后台镜像清理（best-effort）').first(),
    ).toBeVisible()

    const commandToggle = pruneFailureCard
      .getByRole('button', {
        name: '命令输出',
      })
      .first()
    await expect(commandToggle).toBeVisible()
    await commandToggle.click()

    await expect(
      pruneFailureCard.getByText('podman image prune -f'),
    ).toBeVisible()
    await expect(pruneFailureCard.getByText('exit=1')).toBeVisible()
    await expect(
      pruneFailureCard.getByText('mock image prune failure'),
    ).toBeVisible()
  })

  test('groups auto-update warnings and shows warning badge count', async ({ page }) => {
    const rows = await openTasksPage(page)

    const autoUpdateRow = rows
      .filter({ hasText: 'Auto-update run succeeded with warnings' })
      .first()
    await expect(autoUpdateRow).toBeVisible()

    const statusCell = autoUpdateRow.locator('td').nth(1)
    await expect(statusCell.getByText('2')).toBeVisible()

    await autoUpdateRow.click()

    const timeline = page
      .locator('section')
      .filter({ hasText: '日志时间线' })
      .first()
    await expect(timeline).toBeVisible()

    await expect(timeline.getByText('Auto-update warnings (2)')).toBeVisible()
    await expect(
      timeline
        .getByText('auto-update warning for podman-auto-update.service')
        .first(),
    ).toBeVisible()
  })
})
