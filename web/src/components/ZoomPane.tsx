import { memo, useCallback, useEffect, useRef, useState, type PointerEvent as RPointerEvent, type ReactNode, type RefObject } from 'react'
import { fullUrl, previewUrl, thumbUrl } from '@/api/client'
import type { Photo } from '@/api/types'
import { useElementSize } from '@/lib/hooks'
import { cssImageSize, loupeSource, snapRect } from '@/lib/loupeSource'
import {
  clampView,
  imageRect,
  pointToNorm,
  toggleFit100,
  viewAt100,
  zoomAt,
  panBy,
  type ViewState,
} from '@/lib/zoom'

interface Props {
  photo: Photo
  view: ViewState
  onViewChange: (v: ViewState) => void
  /** Hold-to-zoom: when set, shows 100% around this normalized point instead of `view`. */
  hold?: { nx: number; ny: number } | null
  /** Reports the normalized image point under the pointer (for hold-to-zoom + sync). */
  onHover?: (nx: number, ny: number) => void
  /** Touch swipe at fit zoom: +1 = next, -1 = previous. */
  onSwipe?: (dir: 1 | -1) => void
  /** Changing value toggles fit <-> 100% (Space). */
  toggleSignal?: number
  /** Show the zoom badge (Fit / NN%). */
  showZoom?: boolean
  /** Drawn on top of the image, in its coordinate space (normalized 0-1 children via %). */
  overlay?: ReactNode
  /** Fully rendered image (edit page): replaces the thumb/preview layers. */
  src?: string | null
  /** Pixel size of `src` (its aspect may differ from the photo after a crop). */
  imageSize?: { w: number; h: number }
}

/**
 * Layer that fades in when its image has loaded. An image that is already decoded in the browser's
 * cache (a preloaded neighbour in the loupe) is shown at once, without the fade, so stepping through
 * photos does not wait for an animation. `big` (the full-resolution image): fetched first, and faded
 * in only once decoded, so the preview below stays until the full image can actually be drawn.
 */
function Layer({ src, rect, className, big }: { src: string; rect: React.CSSProperties; className?: string; big?: boolean }) {
  const [state, setState] = useState<'pending' | 'instant' | 'fade'>('pending')
  const ref = useCallback((el: HTMLImageElement | null) => {
    if (el && el.complete && el.naturalWidth > 0) setState((s) => (s === 'pending' ? 'instant' : s))
  }, [])
  return (
    <img
      ref={ref}
      src={src}
      alt=""
      draggable={false}
      decoding="async"
      fetchPriority={big ? 'high' : undefined}
      onLoad={(e) => {
        const show = () => setState((s) => (s === 'pending' ? 'fade' : s))
        if (big) void e.currentTarget.decode().then(show, show)
        else show()
      }}
      className={`pointer-events-none absolute max-w-none select-none ${className ?? ''}`}
      style={{ ...rect, opacity: state === 'pending' ? 0 : 1, transition: state === 'fade' ? 'opacity 160ms ease-out' : 'none' }}
    />
  )
}

/** `window.devicePixelRatio`, updated when the window moves to another screen or the page is zoomed. */
function useDevicePixelRatio(): number {
  const [dpr, setDpr] = useState(() => (typeof window === 'undefined' ? 1 : window.devicePixelRatio || 1))
  useEffect(() => {
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return
    const mq = window.matchMedia(`(resolution: ${dpr}dppx)`)
    const update = () => setDpr(window.devicePixelRatio || 1)
    mq.addEventListener('change', update)
    return () => mq.removeEventListener('change', update)
  }, [dpr])
  return dpr
}

/** Position of the element on the page (CSS px), re-measured when it or the window resizes. */
function usePageOrigin(ref: RefObject<HTMLElement | null>): { x: number; y: number } {
  const [origin, setOrigin] = useState({ x: 0, y: 0 })
  useEffect(() => {
    const el = ref.current
    if (!el || typeof ResizeObserver === 'undefined') return
    const measure = () => {
      const b = el.getBoundingClientRect()
      setOrigin((o) => (o.x === b.left && o.y === b.top ? o : { x: b.left, y: b.top }))
    }
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    window.addEventListener('resize', measure)
    return () => {
      ro.disconnect()
      window.removeEventListener('resize', measure)
    }
  }, [ref])
  return origin
}

/**
 * Zoomable/pannable image surface. Progressive: 256 thumb -> 2048 preview -> 4096 preview -> the
 * full-resolution image once zoomed in past the preview's pixels (`loupeSource`). 100% is one image
 * pixel per device pixel. Wheel zooms around the cursor; drag pans; double-click toggles fit/100%.
 */
export const ZoomPane = memo(function ZoomPane({ photo, view, onViewChange, hold, onHover, onSwipe, toggleSignal = 0, showZoom = true, overlay, src, imageSize }: Props) {
  const [ref, size] = useElementSize<HTMLDivElement>()
  const dpr = useDevicePixelRatio()
  const origin = usePageOrigin(ref)
  const natural = imageSize ?? { w: photo.width ?? 3000, h: photo.height ?? 2000 }
  // Photo mode: 100% = one photo pixel per device pixel. The zoom maths work in CSS px, so they get
  // the photo at its CSS size at 100%, and the result is snapped to device pixels. A rendered frame
  // (`src`, edit page) keeps its own pixel scale.
  const img = src ? natural : cssImageSize(natural, dpr)
  const effective = hold ? viewAt100(hold.nx, hold.ny, size, img) : clampView(view, size, img)
  const rect = src ? imageRect(effective, size, img) : snapRect(imageRect(effective, size, img), dpr, origin)
  const rectStyle: React.CSSProperties = { left: rect.left, top: rect.top, width: rect.width, height: rect.height }

  // Latest props for native (non-passive) wheel listener.
  const latest = useRef({ view, size, img, onViewChange })
  useEffect(() => {
    latest.current = { view, size, img, onViewChange }
  })

  const lastSignal = useRef(toggleSignal)
  useEffect(() => {
    if (lastSignal.current === toggleSignal) return
    lastSignal.current = toggleSignal
    const { view: v, size: c, img: im, onViewChange: change } = latest.current
    change(toggleFit100(clampView(v, c, im), c, im))
  }, [toggleSignal])

  useEffect(() => {
    const el = ref.current
    if (!el) return
    const onWheel = (e: WheelEvent) => {
      e.preventDefault()
      const { view: v, size: c, img: im, onViewChange: change } = latest.current
      const b = el.getBoundingClientRect()
      const factor = Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.0016))
      change(zoomAt(clampView(v, c, im), factor, e.clientX - b.left, e.clientY - b.top, c, im))
    }
    el.addEventListener('wheel', onWheel, { passive: false })
    return () => el.removeEventListener('wheel', onWheel)
  }, [ref])

  const drag = useRef<{ x: number; y: number; sx: number; sy: number; touch: boolean } | null>(null)
  const [dragging, setDragging] = useState(false)

  const local = (e: { clientX: number; clientY: number }) => {
    const b = ref.current!.getBoundingClientRect()
    return { x: e.clientX - b.left, y: e.clientY - b.top }
  }

  const onPointerDown = (e: RPointerEvent) => {
    if (e.button !== 0 && e.pointerType === 'mouse') return
    ;(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId)
    const p = local(e)
    drag.current = { x: p.x, y: p.y, sx: p.x, sy: p.y, touch: e.pointerType !== 'mouse' }
    setDragging(true)
  }
  const onPointerMove = (e: RPointerEvent) => {
    const p = local(e)
    if (onHover) {
      const n = pointToNorm(clampView(view, size, img), size, img, p.x, p.y)
      onHover(Math.min(1, Math.max(0, n.nx)), Math.min(1, Math.max(0, n.ny)))
    }
    const d = drag.current
    if (!d || hold) return
    if (view.zoom > 1.001) {
      onViewChange(panBy(clampView(view, size, img), p.x - d.x, p.y - d.y, size, img))
      d.x = p.x
      d.y = p.y
    }
  }
  const onPointerUp = (e: RPointerEvent) => {
    const d = drag.current
    drag.current = null
    setDragging(false)
    if (d?.touch && onSwipe && view.zoom <= 1.001) {
      const p = local(e)
      const dx = p.x - d.sx
      const dy = p.y - d.sy
      if (Math.abs(dx) > 50 && Math.abs(dx) > Math.abs(dy) * 1.5) onSwipe(dx < 0 ? 1 : -1)
    }
  }
  const onDouble = (e: React.MouseEvent) => {
    const p = local(e)
    onViewChange(toggleFit100(clampView(view, size, img), size, img, p.x, p.y))
  }

  const zoomed = effective.zoom > 1.001
  const nativeLong = photo.width && photo.height ? Math.max(photo.width, photo.height) : null
  const source = src ? null : loupeSource(effective.zoom, Math.max(rect.width, rect.height) * dpr, nativeLong)
  // The 4096 preview is a step on the way when zooming in gradually and stays while the full image
  // loads. Jumping straight to 100% (Space, double-click, Z) fetches only the full image: for a
  // large photo the preview costs the server about as much and would arrive barely earlier.
  const [hiShown, setHiShown] = useState<number | null>(null)
  if (source === 4096 && hiShown !== photo.id) setHiShown(photo.id)
  const showHi = source === 4096 || (source === 'full' && hiShown === photo.id)

  return (
    <div
      ref={ref}
      className="relative h-full w-full touch-none overflow-hidden bg-black/30"
      style={{ cursor: hold ? 'zoom-in' : zoomed ? (dragging ? 'grabbing' : 'grab') : 'default' }}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onDoubleClick={onDouble}
      data-testid="zoom-pane"
    >
      {showZoom && size.w > 0 && (
        <div className="tnum pointer-events-none absolute top-2 right-2 rounded bg-black/60 px-1.5 py-0.5 text-xs text-white/80">
          {zoomed ? `${Math.round((rect.width / img.w) * 100)}%` : 'Fit'}
        </div>
      )}
      {size.w > 0 && (
        <>
          {src ? (
            <img
              src={src}
              alt=""
              draggable={false}
              className="pointer-events-none absolute max-w-none select-none"
              style={rectStyle}
              data-testid="render-frame"
            />
          ) : (
            <>
              <Layer key={`t${photo.id}`} src={thumbUrl(photo, 256)} rect={rectStyle} className="[image-rendering:auto]" />
              <Layer key={`p${photo.id}`} src={previewUrl(photo.id, 2048, photo.thumb_version)} rect={rectStyle} />
              {showHi && <Layer key={`h${photo.id}`} src={previewUrl(photo.id, 4096, photo.thumb_version)} rect={rectStyle} />}
              {source === 'full' && <Layer key={`f${photo.id}`} src={fullUrl(photo.id, photo.thumb_version)} rect={rectStyle} big />}
            </>
          )}
          {overlay && (
            <div className="pointer-events-none absolute" style={rectStyle}>
              {overlay}
            </div>
          )}
        </>
      )}
    </div>
  )
})
