import { expect } from '@storybook/test'
import { setProjectAnnotations } from '@storybook/react'
import * as projectAnnotations from './preview'

setProjectAnnotations(projectAnnotations)

// Make Storybook's expect (with testing-library matchers) available globally.
;(globalThis as typeof globalThis & { expect: typeof expect }).expect = expect
