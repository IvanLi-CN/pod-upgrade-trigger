import { defineConfig, devices } from '@playwright/test'

const baseURL = process.env.UI_E2E_BASE_URL || 'http://127.0.0.1:25211'

export default defineConfig({
  testDir: 'tests/ui',
  timeout: 30_000,
  expect: {
    timeout: 10_000,
  },
  retries: process.env.CI ? 1 : 0,
  reporter: [
    ['list'],
    ['html', { outputFolder: 'playwright-report', open: 'never' }],
  ],
  use: {
    ...devices['Desktop Chrome'],
    baseURL,
    trace: 'on-first-retry',
    video: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
})

