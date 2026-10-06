/**
 * Progressive preview scheduling (docs/api-contract-m3.md, POST /api/render/preview).
 *  - while a control is dragged: small frames (`long_edge` ~ 800), only the LATEST request is kept,
 *    a superseded in-flight request is aborted (AbortController) and its result discarded;
 *  - on release: one screen-resolution request (devicePixelRatio aware) which also supersedes drag frames.
 * The consumer keeps showing the last good frame, so nothing flickers while requests are in flight.
 */

export const DRAG_LONG_EDGE = 800
export const MAX_LONG_EDGE = 4096

export interface PreviewRequest {
  /** identity of the render (stack + photo + mode); equal keys are never re-requested back to back */
  key: string
  longEdge: number
  /** true = release / settled frame */
  final: boolean
  /** opaque payload handed to the fetcher */
  payload: unknown
}

export interface Frame<T> {
  request: PreviewRequest
  data: T
}

export type Fetcher<T> = (req: PreviewRequest, signal: AbortSignal) => Promise<T>

export interface SchedulerOptions<T> {
  fetcher: Fetcher<T>
  onFrame: (f: Frame<T>) => void
  onError?: (err: unknown, req: PreviewRequest) => void
  /** minimum spacing between drag-frame requests (ms) */
  minIntervalMs?: number
  now?: () => number
  setTimer?: (fn: () => void, ms: number) => unknown
  clearTimer?: (h: unknown) => void
}

/** Long edge to request for a drag frame or the settled frame. */
export function previewLongEdge(final: boolean, container: { w: number; h: number }, dpr: number): number {
  if (!final) return DRAG_LONG_EDGE
  const px = Math.round(Math.max(container.w, container.h) * Math.max(1, dpr))
  // quantise to 64 px so tiny resizes do not trigger new renders
  const q = Math.ceil(px / 64) * 64
  return Math.min(MAX_LONG_EDGE, Math.max(DRAG_LONG_EDGE, q))
}

export class PreviewScheduler<T> {
  private opts: Required<Pick<SchedulerOptions<T>, 'minIntervalMs' | 'now' | 'setTimer' | 'clearTimer'>> & SchedulerOptions<T>
  private inflight: { req: PreviewRequest; ctrl: AbortController; gen: number } | null = null
  private pending: PreviewRequest | null = null
  private timer: unknown = null
  private lastStart = -Infinity
  private gen = 0
  private lastShown: { key: string; longEdge: number } | null = null
  private disposed = false
  /** number of requests actually started (for tests / dev badge) */
  started = 0
  aborted = 0

  constructor(opts: SchedulerOptions<T>) {
    this.opts = {
      minIntervalMs: 40,
      now: () => Date.now(),
      setTimer: (fn, ms) => setTimeout(fn, ms),
      clearTimer: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
      ...opts,
    }
  }

  get busy(): boolean {
    return this.inflight !== null || this.pending !== null
  }

  /** Ask for a frame. Replaces any queued request; aborts a superseded in-flight one. */
  request(req: PreviewRequest): void {
    if (this.disposed) return
    // Already rendering / showing exactly this? Nothing to do (final upgrades a drag frame of the same key).
    const inflight = this.inflight?.req
    if (inflight && inflight.key === req.key && inflight.longEdge >= req.longEdge) {
      this.pending = null
      this.clearPending()
      return
    }
    // The frame on screen already shows this render at an equal or better resolution.
    if (!inflight && this.lastShown && this.lastShown.key === req.key && this.lastShown.longEdge >= req.longEdge) return
    this.pending = req
    if (req.final) {
      // Releasing a control: supersede everything and go now.
      this.abortInflight()
      this.start()
      return
    }
    const wait = this.lastStart + this.opts.minIntervalMs - this.opts.now()
    if (wait <= 0) {
      this.abortInflight()
      this.start()
    } else if (this.timer === null) {
      this.timer = this.opts.setTimer(() => {
        this.timer = null
        this.abortInflight()
        this.start()
      }, wait)
    }
  }

  cancel(): void {
    this.pending = null
    this.clearPending()
    this.abortInflight()
  }

  dispose(): void {
    this.disposed = true
    this.cancel()
  }

  private clearPending() {
    if (this.timer !== null) {
      this.opts.clearTimer(this.timer)
      this.timer = null
    }
  }

  private abortInflight() {
    if (this.inflight) {
      this.inflight.ctrl.abort()
      this.aborted++
      this.inflight = null
    }
  }

  private start() {
    this.clearPending()
    const req = this.pending
    if (!req || this.disposed) return
    this.pending = null
    const ctrl = new AbortController()
    const gen = ++this.gen
    this.inflight = { req, ctrl, gen }
    this.lastStart = this.opts.now()
    this.started++
    this.opts
      .fetcher(req, ctrl.signal)
      .then((data) => {
        // Superseded (or cancelled) while in flight: drop silently.
        if (this.disposed || ctrl.signal.aborted || this.gen !== gen) return
        this.inflight = null
        this.lastShown = { key: req.key, longEdge: req.longEdge }
        this.opts.onFrame({ request: req, data })
        // A newer request queued up behind this one (throttled): start it now.
        if (this.pending && this.timer === null) this.start()
      })
      .catch((err: unknown) => {
        if (ctrl.signal.aborted || this.gen !== gen) return
        this.inflight = null
        this.opts.onError?.(err, req)
        if (this.pending && this.timer === null) this.start()
      })
  }
}
