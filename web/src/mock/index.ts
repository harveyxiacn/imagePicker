import { setupWorker } from 'msw/browser'
import { socketConfig } from '@/api/events'
import { MockSocket } from './bus'
import { seedMockDb } from './db'
import { handlers } from './handlers'

/** Starts the in-browser mock backend (MSW + fake WebSocket event stream). */
export async function enableMock(): Promise<void> {
  seedMockDb()
  socketConfig.factory = () => new MockSocket()
  await setupWorker(...handlers).start({
    onUnhandledFrame: 'bypass',
    quiet: true,
    serviceWorker: { url: `${import.meta.env.BASE_URL}mockServiceWorker.js` },
  })
}
