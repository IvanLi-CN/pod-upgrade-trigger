import type { IncomingMessage, ServerResponse } from 'node:http'
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react-swc'
import { runtime } from './src/mocks/runtime'

async function bufferBody(req: IncomingMessage) {
  const chunks: Buffer[] = []
  for await (const chunk of req) {
    chunks.push(Buffer.from(chunk))
  }
  return Buffer.concat(chunks)
}

async function handleNodeMock(req: IncomingMessage, res: ServerResponse) {
  if (process.env.VITE_ENABLE_MOCKS !== 'true') return false
  const url = req.url ?? ''
  const method = (req.method ?? 'GET').toUpperCase()

  if (method === 'POST' && url.startsWith('/github-package-update/')) {
    const bytes = new Uint8Array(await bufferBody(req))
    runtime.storePayload(bytes)
    const slug = url.split('/').pop() ?? 'unknown'
    const ts = Math.floor(Date.now() / 1000)
    runtime.updateWebhook(slug, {
      hmac_ok: false,
      hmac_last_error: 'signature mismatch',
      last_ts: ts,
      last_failure_ts: ts,
      last_request_id: `hw-${Date.now()}`,
      last_status: 401,
    })
    runtime.addEvent({
      request_id: `hw-${Date.now()}`,
      ts,
      method: 'POST',
      path: url,
      status: 401,
      action: 'github-webhook',
      duration_ms: 90,
      meta: { source: 'node-middleware' },
    })
    res.statusCode = 200
    res.setHeader('Content-Type', 'application/json')
    res.end(JSON.stringify({ status: 'recorded' }))
    return true
  }

  if (method === 'GET' && url.startsWith('/last_payload.bin')) {
    const buffer = Buffer.from(runtime.cloneData().lastPayload)
    res.statusCode = 200
    res.setHeader('Content-Type', 'application/octet-stream')
    res.setHeader('Content-Length', buffer.byteLength)
    res.end(buffer)
    return true
  }

  return false
}

// https://vite.dev/config/
export default defineConfig({
  plugins: [
    {
      name: 'mock-middleware',
      configureServer(server) {
        server.middlewares.use(async (req, res, next) => {
          const handled = await handleNodeMock(req, res)
          if (!handled) {
            next()
          }
        })
      },
    },
    react(),
  ],
})
