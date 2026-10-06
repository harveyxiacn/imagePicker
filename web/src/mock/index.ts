import { setupWorker } from 'msw/browser'
import { socketConfig } from '@/api/events'
import { emit, MockSocket } from './bus'
import { seedMockDb } from './db'
import { handlers } from './handlers'
import { seedM6Mock } from './m6'

/** Starts the in-browser mock backend (MSW + fake WebSocket event stream). */
export async function enableMock(): Promise<void> {
  seedMockDb()
  seedM6Mock()
  // test hook: lets browser tests push server events (e.g. a CPU tier `worker.status`)
  ;(globalThis as { __mockEmit?: typeof emit }).__mockEmit = emit
  socketConfig.factory = () => new MockSocket()
  await setupWorker(...handlers).start({
    onUnhandledFrame: 'bypass',
    quiet: true,
    serviceWorker: { url: `${import.meta.env.BASE_URL}mockServiceWorker.js` },
  })
}
