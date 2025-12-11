import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { defineConfig, mergeConfig } from 'vitest/config'
import { storybookTest } from '@storybook/experimental-addon-test/vitest-plugin'
import viteConfig from './vite.config'

const dirname =
  typeof __dirname !== 'undefined' ? __dirname : path.dirname(fileURLToPath(import.meta.url))

const storybookPlugins = await storybookTest({
  configDir: path.join(dirname, '.storybook'),
})

export default mergeConfig(
  viteConfig,
  defineConfig({
    plugins: [...storybookPlugins],
    test: {
      name: 'storybook',
      browser: {
        enabled: true,
        provider: 'playwright',
        instances: [{ browser: 'chromium' }],
      },
      setupFiles: ['./.storybook/vitest.setup.ts'],
    },
  }),
)
