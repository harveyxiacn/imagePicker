import { QueryClient } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { EVENT_DRIVEN_KEYS } from '@/lib/cache'
import { socketConfig, startEvents, type SocketLike } from './events'

class FakeSocket implements SocketLike {
  onopen: ((ev: Event) => void) | null = null
  onmessage: ((ev: MessageEvent) => void) | null = null
  onclose: ((ev: CloseEvent) => void) | null = null
  onerror: ((ev: Event) => void) | null = null
  closed = false
  close() {
    this.closed = true
    this.onclose?.({} as CloseEvent)
  }
  open() {
    this.onopen?.({} as Event)
  }
  send(ev: unknown) {
    this.onmessage?.({ data: JSON.stringify(ev) } as MessageEvent)
  }
  drop() {
    this.onclose?.({} as CloseEvent)
  }
}

const original = socketConfig.factory
let sockets: FakeSocket[]
let times: number[]

beforeEach(() => {
  vi.useFakeTimers()
  vi.stubGlobal('location', { protocol: 'http:', host: 'h' })
  sockets = []
  times = []
  socketConfig.factory = () => {
    const s = new FakeSocket()
    sockets.push(s)
    times.push(Date.now())
    return s
  }
})

afterEach(() => {
  socketConfig.factory = original
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

const task = (done: number, id = 'e1') => ({ type: 'task.progress', task_id: id, kind: 'export', total: 10, done, state: 'running' })

describe('startEvents', () => {
  it('coalesces frames within the flush window and forwards the latest task.progress once', () => {
    const onTask = vi.fn()
    const stop = startEvents(new QueryClient(), { onTask, flushMs: 100 })
    sockets[0].open()
    sockets[0].send(task(1))
    sockets[0].send(task(2))
    sockets[0].send(task(3, 'e2'))
    expect(onTask).not.toHaveBeenCalled()
    vi.advanceTimersByTime(100)
    expect(onTask.mock.calls.map((c) => [c[0].task_id, c[0].done])).toEqual([['e1', 2], ['e2', 3]])
    sockets[0].send(task(4))
    vi.advanceTimersByTime(100)
    expect(onTask).toHaveBeenCalledTimes(3)
    stop()
  })

  it('ignores frames that are not JSON', () => {
    const onTask = vi.fn()
    const stop = startEvents(new QueryClient(), { onTask })
    sockets[0].open()
    sockets[0].onmessage?.({ data: 'not json' } as MessageEvent)
    vi.advanceTimersByTime(1000)
    expect(onTask).not.toHaveBeenCalled()
    stop()
  })

  it('reconnects with a doubling backoff capped at 10 s, and resets it after an open', () => {
    const stop = startEvents(new QueryClient())
    const delays: number[] = []
    for (let i = 0; i < 7; i++) {
      const n = sockets.length
      const before = Date.now()
      sockets[n - 1].drop()
      vi.advanceTimersByTime(20_000)
      expect(sockets.length).toBe(n + 1)
      delays.push(times[n] - before)
    }
    expect(delays).toEqual([500, 1000, 2000, 4000, 8000, 10_000, 10_000])
    // a successful open resets the attempt counter
    sockets[sockets.length - 1].open()
    const n = sockets.length
    const before = Date.now()
    sockets[n - 1].drop()
    vi.advanceTimersByTime(20_000)
    expect(times[n] - before).toBe(500)
    stop()
  })

  it('reports connection status', () => {
    const onStatus = vi.fn()
    const stop = startEvents(new QueryClient(), { onStatus })
    sockets[0].open()
    sockets[0].drop()
    expect(onStatus.mock.calls.map((c) => c[0])).toEqual(['connecting', 'open', 'closed'])
    stop()
  })

  it('resyncs only after a reconnect, and only the event-driven query families', () => {
    const qc = new QueryClient()
    const keys: unknown[][] = [
      ['photos', 1, {}],
      ['session', 1],
      ['groups', 1],
      ['analysis', 'status', 1],
      ['tasks'],
      ['presets'],
      ['luts'],
      ['auth', 'me'],
      ['import', 'sources'],
      ['remote', 'status'],
      ['cache'],
    ]
    for (const k of keys) qc.setQueryData(k, 1)
    const stale = () => keys.filter((k) => qc.getQueryState(k)?.isInvalidated).map((k) => String(k[0]))
    const stop = startEvents(qc)
    sockets[0].open()
    expect(stale()).toEqual([]) // first connect: nothing was missed
    sockets[0].drop()
    vi.advanceTimersByTime(1000)
    sockets[1].open()
    expect(stale()).toEqual(['photos', 'session', 'groups', 'analysis', 'tasks'])
    for (const k of keys) expect(EVENT_DRIVEN_KEYS.has(String(k[0])) === (stale().includes(String(k[0])))).toBe(true)
    stop()
  })

  it('does not reconnect after dispose, and closes the live socket without a status flap', () => {
    const onStatus = vi.fn()
    const stop = startEvents(new QueryClient(), { onStatus })
    sockets[0].open()
    stop()
    expect(sockets[0].closed).toBe(true)
    vi.advanceTimersByTime(60_000)
    expect(sockets).toHaveLength(1)
    expect(onStatus.mock.calls.map((c) => c[0])).toEqual(['connecting', 'open'])
  })

  it('dispose cancels a pending reconnect and a pending flush', () => {
    const onTask = vi.fn()
    const stop = startEvents(new QueryClient(), { onTask })
    sockets[0].open()
    sockets[0].send(task(1))
    sockets[0].drop() // retry scheduled
    stop()
    vi.advanceTimersByTime(60_000)
    expect(sockets).toHaveLength(1)
    expect(onTask).not.toHaveBeenCalled()
  })
})
