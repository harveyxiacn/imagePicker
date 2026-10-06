import { describe, expect, it } from 'vitest'
import { DRAG_LONG_EDGE, MAX_LONG_EDGE, PreviewScheduler, previewLongEdge, type Frame, type PreviewRequest } from './previewScheduler'

interface Call {
  req: PreviewRequest
  signal: AbortSignal
  resolve: (v: string) => void
  reject: (e: unknown) => void
}

/** Scheduler wired to a manual clock and a controllable fetcher. */
function setup(minIntervalMs = 40) {
  let now = 1000
  const timers: { at: number; fn: () => void; id: number }[] = []
  let nextId = 1
  const calls: Call[] = []
  const frames: Frame<string>[] = []
  const errors: unknown[] = []
  const sched = new PreviewScheduler<string>({
    minIntervalMs,
    now: () => now,
    setTimer: (fn, ms) => {
      const id = nextId++
      timers.push({ at: now + ms, fn, id })
      return id
    },
    clearTimer: (h) => {
      const i = timers.findIndex((t) => t.id === h)
      if (i >= 0) timers.splice(i, 1)
    },
    fetcher: (req, signal) =>
      new Promise<string>((resolve, reject) => {
        calls.push({ req, signal, resolve, reject })
        signal.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')))
      }),
    onFrame: (f) => frames.push(f),
    onError: (e) => errors.push(e),
  })
  const advance = (ms: number) => {
    now += ms
    for (const t of timers.filter((x) => x.at <= now).sort((a, b) => a.at - b.at)) {
      timers.splice(timers.indexOf(t), 1)
      t.fn()
    }
  }
  const flush = () => new Promise((r) => setTimeout(r, 0))
  return { sched, calls, frames, errors, advance, flush }
}

const req = (key: string, final: boolean, longEdge = final ? 1600 : DRAG_LONG_EDGE): PreviewRequest => ({ key, final, longEdge, payload: { key } })

describe('previewLongEdge', () => {
  it('uses ~800 px while dragging regardless of the screen', () => {
    expect(previewLongEdge(false, { w: 1600, h: 900 }, 2)).toBe(DRAG_LONG_EDGE)
  })

  it('uses the screen resolution (devicePixelRatio aware) on release, quantised to 64 px', () => {
    expect(previewLongEdge(true, { w: 1000, h: 700 }, 1)).toBe(1024)
    expect(previewLongEdge(true, { w: 1000, h: 700 }, 2)).toBe(2048)
    expect(previewLongEdge(true, { w: 1107, h: 700 }, 1.5)).toBe(1664)
  })

  it('clamps to [800, 4096]', () => {
    expect(previewLongEdge(true, { w: 200, h: 100 }, 1)).toBe(DRAG_LONG_EDGE)
    expect(previewLongEdge(true, { w: 0, h: 0 }, 1)).toBe(DRAG_LONG_EDGE)
    expect(previewLongEdge(true, { w: 3840, h: 2160 }, 2)).toBe(MAX_LONG_EDGE)
  })
})

describe('PreviewScheduler', () => {
  it('starts the first drag request immediately and delivers its frame', async () => {
    const t = setup()
    t.sched.request(req('a', false))
    expect(t.calls).toHaveLength(1)
    expect(t.calls[0].req.longEdge).toBe(DRAG_LONG_EDGE)
    t.calls[0].resolve('frame-a')
    await t.flush()
    expect(t.frames.map((f) => f.data)).toEqual(['frame-a'])
  })

  it('aborts a superseded in-flight request and drops its result (latest wins)', async () => {
    const t = setup()
    t.sched.request(req('a', false))
    t.advance(50)
    t.sched.request(req('b', false))
    expect(t.calls).toHaveLength(2)
    expect(t.calls[0].signal.aborted).toBe(true)
    expect(t.calls[1].signal.aborted).toBe(false)
    // even if the old one somehow resolves, it is never shown
    t.calls[0].resolve('old')
    t.calls[1].resolve('new')
    await t.flush()
    expect(t.frames.map((f) => f.data)).toEqual(['new'])
    expect(t.errors).toEqual([])
    expect(t.sched.aborted).toBe(1)
  })

  it('throttles bursts of drag requests: only the newest queued one runs', async () => {
    const t = setup(40)
    t.sched.request(req('a', false))
    t.sched.request(req('b', false))
    t.sched.request(req('c', false))
    expect(t.calls).toHaveLength(1) // b and c are held back
    t.advance(40)
    expect(t.calls).toHaveLength(2)
    expect(t.calls[1].req.key).toBe('c')
    expect(t.calls[0].signal.aborted).toBe(true)
    t.calls[1].resolve('c')
    await t.flush()
    expect(t.frames.map((f) => f.request.key)).toEqual(['c'])
  })

  it('a release request supersedes a drag frame immediately at screen resolution', async () => {
    const t = setup()
    t.sched.request(req('a', false))
    t.sched.request(req('a', true, 2048))
    expect(t.calls).toHaveLength(2)
    expect(t.calls[0].signal.aborted).toBe(true)
    expect(t.calls[1].req).toMatchObject({ final: true, longEdge: 2048 })
    t.calls[1].resolve('final')
    await t.flush()
    expect(t.frames.map((f) => [f.request.final, f.data])).toEqual([[true, 'final']])
  })

  it('does not re-request what is already on screen at the same or better resolution', async () => {
    const t = setup()
    t.sched.request(req('a', true, 1600))
    t.calls[0].resolve('x')
    await t.flush()
    t.sched.request(req('a', false)) // drag frame of the same render: nothing to do
    t.sched.request(req('a', true, 1600))
    expect(t.calls).toHaveLength(1)
    t.sched.request(req('a', true, 2048)) // bigger window: upgrade
    expect(t.calls).toHaveLength(2)
  })

  it('ignores the duplicate of an in-flight request', () => {
    const t = setup()
    t.sched.request(req('a', true, 1600))
    t.sched.request(req('a', true, 1600))
    expect(t.calls).toHaveLength(1)
    expect(t.calls[0].signal.aborted).toBe(false)
  })

  it('reports real failures but swallows aborts', async () => {
    const t = setup()
    t.sched.request(req('a', true))
    t.calls[0].reject(new Error('boom'))
    await t.flush()
    expect(t.errors).toHaveLength(1)
    t.advance(100)
    t.sched.request(req('b', false))
    t.sched.cancel()
    await t.flush()
    expect(t.errors).toHaveLength(1)
    expect(t.calls[1].signal.aborted).toBe(true)
  })

  it('dispose aborts in-flight work and ignores later requests', async () => {
    const t = setup()
    t.sched.request(req('a', true))
    t.sched.dispose()
    expect(t.calls[0].signal.aborted).toBe(true)
    t.sched.request(req('b', true))
    expect(t.calls).toHaveLength(1)
    t.calls[0].resolve('late')
    await t.flush()
    expect(t.frames).toEqual([])
  })
})
