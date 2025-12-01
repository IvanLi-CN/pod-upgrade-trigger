import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'

async function bootstrap() {
  const rootElement = document.getElementById('root')
  if (!rootElement) {
    throw new Error('Failed to locate root element')
  }

  const enableMocks =
    import.meta.env.VITE_ENABLE_MOCKS === 'true' ||
    window.location.search.includes('mock')

  if (enableMocks) {
    const { startMocks } = await import('./mocks/browser')
    await startMocks()
  }

  createRoot(rootElement).render(
    <StrictMode>
      <App mockEnabled={enableMocks} />
    </StrictMode>,
  )
}

bootstrap()
