import type { Meta, StoryObj } from '@storybook/react'
import { useEffect } from 'react'
import { ToastProvider, useToast, type ToastOptions } from './Toast'

type ToastStoryProps = {
  initialToasts?: ToastOptions[]
}

function ToastInitializer({ toasts }: { toasts: ToastOptions[] }) {
  const { pushToast } = useToast()
  useEffect(() => {
    toasts.forEach((toast, idx) => {
      pushToast({ ...toast, id: toast.id ?? `seed-${idx}` })
    })
  }, [toasts, pushToast])
  return null
}

function ToastStory({ initialToasts = [] }: ToastStoryProps) {
  return (
    <ToastProvider>
      <ToastInitializer toasts={initialToasts} />
      <div className="space-y-2 p-4">
        <p className="text-sm text-base-content/80">
          ToastProvider renders alerts in a fixed viewport. Stories seed example toasts via
          `initialToasts`.
        </p>
      </div>
    </ToastProvider>
  )
}

const meta: Meta<typeof ToastStory> = {
  title: 'Components/Toast',
  component: ToastStory,
  tags: ['autodocs'],
  args: {
    initialToasts: [],
  },
}

export default meta
type Story = StoryObj<typeof ToastStory>

export const PrefilledSuccess: Story = {
  name: 'Prefilled success',
  args: {
    initialToasts: [
      {
        variant: 'success',
        title: 'Deployment complete',
        message: 'All pods were updated successfully.',
      },
    ],
  },
}

export const PrefilledError: Story = {
  name: 'Prefilled error',
  args: {
    initialToasts: [
      {
        variant: 'error',
        title: 'Upgrade failed',
        message: 'Health check timed out during rollout.',
      },
    ],
  },
}

export const EmptyQueue: Story = {
  name: 'Empty queue',
  args: {
    initialToasts: [],
  },
}
