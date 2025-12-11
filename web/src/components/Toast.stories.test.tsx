import { afterEach, describe, it } from 'vitest'
import { composeStories } from '@storybook/react'
import { render, cleanup } from '@testing-library/react'
import { expect, screen } from '@storybook/test'
import * as ToastStories from './Toast.stories'

const { PrefilledSuccess, PrefilledError, EmptyQueue } = composeStories(ToastStories)

afterEach(() => cleanup())

describe('Toast stories', () => {
  it('renders a success toast when seeded', async () => {
    render(<PrefilledSuccess />)

    await screen.findByText('Deployment complete')
    expect(
      screen.getByText('All pods were updated successfully.'),
    ).toBeInTheDocument()
  })

  it('renders an error toast with error styling', async () => {
    render(<PrefilledError />)

    const title = await screen.findByText('Upgrade failed')
    const alert = title.closest('.alert')
    expect(alert?.className).toMatch(/alert-error/)
    expect(screen.getByText(/Health check timed out/)).toBeInTheDocument()
  })

  it('shows no toast items for the empty queue story', () => {
    render(<EmptyQueue />)
    expect(document.querySelector('.alert')).toBeNull()
  })
})
