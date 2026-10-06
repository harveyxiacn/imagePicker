import type { QueryClient } from '@tanstack/react-query'
import { applyEvents, type TaskEvent } from '@/lib/cache'
import type { ServerEvent } from './types'

export interface SocketLike {
  onopen: ((ev: Event) => void) | null
  onmessage: ((ev: MessageEvent) => void) | null
  onclose: ((ev: CloseEvent) => void) | null
  onerror: ((ev: Event) => void) | null
  close(): void
}

/** Overridable so the mock backend can supply an in-memory socket. */
export const socketConfig: { factory: (url: string) => SocketLike } = {
  factory: (url) => new WebSocket(url) as unknown as SocketLike,
}

export type ConnectionStatus = 'connecting' | 'open' | 'closed'

export interface EventsOptions {
  onTask?: (e: TaskEvent) => void
  onStatus?: (s: ConnectionStatus) => void
  /** Coalescing window in ms (server also coalesces at 100ms). */
  flushMs?: number
}

export function eventsUrl(): string {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws'
  return `${proto}://${location.host}/api/events`
}

/** Connects to /api/events, buffers frames and applies them to the query cache. Returns a disposer. */
export function startEvents(qc: QueryClient, opts: EventsOptions = {}): () => void {
  const flushMs = opts.flushMs ?? 100
  let buffer: ServerEvent[] = []
  let timer: ReturnType<typeof setTimeout> | null = null
  let socket: SocketLike | null = null
  let retry: ReturnType<typeof setTimeout> | null = null
  let attempt = 0
  let disposed = false
  let everOpened = false

  const flush = () => {
    timer = null
    if (!buffer.length) return
    const batch = buffer
    buffer = []
    applyEvents(qc, batch, opts.onTask)
  }

  const connect = () => {
    if (disposed) return
    opts.onStatus?.('connecting')
    const s = socketConfig.factory(eventsUrl())
    socket = s
    s.onopen = () => {
      attempt = 0
      opts.onStatus?.('open')
      // After a reconnect we may have missed events: resync everything.
      if (everOpened) void qc.invalidateQueries()
      everOpened = true
    }
    s.onmessage = (m) => {
      try {
        buffer.push(JSON.parse(String(m.data)) as ServerEvent)
      } catch {
        return
      }
      if (!timer) timer = setTimeout(flush, flushMs)
    }
    s.onclose = () => {
      opts.onStatus?.('closed')
      socket = null
      if (disposed) return
      const delay = Math.min(10_000, 500 * 2 ** attempt++)
      retry = setTimeout(connect, delay)
    }
    s.onerror = () => s.close()
  }

  connect()
  return () => {
    disposed = true
    if (timer) clearTimeout(timer)
    if (retry) clearTimeout(retry)
    if (socket) {
      socket.onclose = null
      socket.close()
    }
  }
}
