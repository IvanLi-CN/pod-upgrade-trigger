import type { Decorator, Preview } from '@storybook/react'

import { runtime } from '../src/mocks/runtime'
import { startMocks } from '../src/mocks/browser'
import '../src/index.css'

let mocksPromise: Promise<unknown> | null = null

const withMocks: Decorator = (Story) => {
  if (typeof window !== 'undefined') {
    const w = window as typeof window & { __MOCK_ENABLED__?: boolean }
    const alreadyEnabled = w.__MOCK_ENABLED__ === true
    if (!alreadyEnabled && !mocksPromise) {
      mocksPromise = startMocks()
        .then(() => {
          try {
            runtime.resetData('happy-path')
          } catch (err) {
            console.warn('[storybook-msw] failed to reset mock runtime', err)
          }
        })
        .catch((err) => {
          console.error('[storybook-msw] failed to start mocks', err)
          mocksPromise = null
        })
    }
  }

  return <Story />
}

const preview: Preview = {
  parameters: {
    actions: { argTypesRegex: '^on[A-Z].*' },
    controls: {
      matchers: {
        color: /(background|color)$/i,
        date: /Date$/,
      },
    },
    layout: 'centered',
  },
  decorators: [withMocks],
}

export default preview
