import type { SocketLike } from '@/api/events'
import type { ServerEvent } from '@/api/types'

type Listener = (ev: ServerEvent) => void
const listeners = new Set<Listener>()

/** Broadcast a server event to every connected fake WebSocket. */
export function emit(ev: ServerEvent): void {
  for (const l of listeners) l(ev)
}

/** In-memory stand-in for the /api/events WebSocket. */
export class MockSocket implements SocketLike {
  onopen: ((ev: Event) => void) | null = null
  onmessage: ((ev: MessageEvent) => void) | null = null
  onclose: ((ev: CloseEvent) => void) | null = null
  onerror: ((ev: Event) => void) | null = null
  private listener: Listener
  private closed = false

  constructor() {
    this.listener = (ev) => {
      if (this.closed) return
      this.onmessage?.({ data: JSON.stringify(ev) } as MessageEvent)
    }
    setTimeout(() => {
      if (this.closed) return
      listeners.add(this.listener)
      this.onopen?.(new Event('open'))
    }, 30)
  }

  close(): void {
    if (this.closed) return
    this.closed = true
    listeners.delete(this.listener)
    this.onclose?.({} as CloseEvent)
  }
}
