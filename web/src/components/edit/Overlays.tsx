import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useRef, type PointerEvent as RPointerEvent } from 'react'
import { useTranslation } from 'react-i18next'
import { fetchMask } from '@/api/client'
import type { CropOp, LocalOp, MaskRef, Point } from '@/api/types'
import { clamp, dragCropRect, updateLocal, type CropHandle } from '@/lib/edit'
import { qk } from '@/lib/cache'
import { commitLive, setLive } from '@/lib/editActions'
import { maskValue } from '@/lib/maskgeom'
import { useEdit } from '@/stores/edit'

type Rect = CropOp['rect']

/** Pointer drag on an overlay handle: reports normalised coordinates relative to `root`. */
function beginDrag(
  e: RPointerEvent<Element>,
  root: HTMLElement,
  onMove: (nx: number, ny: number, dx: number, dy: number) => void,
  onEnd: () => void,
) {
  e.stopPropagation()
  e.preventDefault()
  const el = e.currentTarget as HTMLElement
  el.setPointerCapture(e.pointerId)
  useEdit.getState().setDragging(true)
  const box = root.getBoundingClientRect()
  const sx = e.clientX
  const sy = e.clientY
  const move = (ev: PointerEvent) => onMove((ev.clientX - box.left) / box.width, (ev.clientY - box.top) / box.height, (ev.clientX - sx) / box.width, (ev.clientY - sy) / box.height)
  const up = () => {
    el.removeEventListener('pointermove', move)
    el.removeEventListener('pointerup', up)
    el.removeEventListener('pointercancel', up)
    useEdit.getState().setDragging(false)
    onEnd()
  }
  el.addEventListener('pointermove', move)
  el.addEventListener('pointerup', up)
  el.addEventListener('pointercancel', up)
}

const stop = (e: { stopPropagation: () => void }) => e.stopPropagation()

// ---------------------------------------------------------------- split before / after

export function SplitOverlay({ before, pos, onPos }: { before: string | null; pos: number; onPos: (n: number) => void }) {
  const { t } = useTranslation()
  const root = useRef<HTMLDivElement>(null)
  return (
    <div ref={root} className="absolute inset-0 overflow-hidden" data-testid="split-overlay">
      {before && (
        <img
          src={before}
          alt=""
          draggable={false}
          className="pointer-events-none absolute inset-0 h-full w-full max-w-none select-none"
          style={{ clipPath: `inset(0 ${(1 - pos) * 100}% 0 0)` }}
        />
      )}
      <span className="pointer-events-none absolute top-2 left-2 rounded bg-black/60 px-1.5 py-0.5 text-[11px] text-white/85">{t('edit.before')}</span>
      <span className="pointer-events-none absolute top-2 right-2 rounded bg-black/60 px-1.5 py-0.5 text-[11px] text-white/85">{t('edit.after')}</span>
      <div className="pointer-events-none absolute top-0 bottom-0 w-px bg-white/90 shadow" style={{ left: `${pos * 100}%` }} />
      <div
        className="pointer-events-auto absolute top-0 bottom-0 flex w-5 -translate-x-1/2 cursor-ew-resize items-center justify-center"
        style={{ left: `${pos * 100}%` }}
        data-testid="split-handle"
        onPointerDown={(e) => root.current && beginDrag(e, root.current, (nx) => onPos(nx), () => undefined)}
        onDoubleClick={stop}
      >
        <span className="flex h-7 w-4 items-center justify-center rounded-full bg-white text-[10px] text-black shadow">⇆</span>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------- crop tool

const HANDLES: { h: CropHandle; x: number; y: number; cursor: string }[] = [
  { h: 'nw', x: 0, y: 0, cursor: 'nwse-resize' },
  { h: 'n', x: 0.5, y: 0, cursor: 'ns-resize' },
  { h: 'ne', x: 1, y: 0, cursor: 'nesw-resize' },
  { h: 'e', x: 1, y: 0.5, cursor: 'ew-resize' },
  { h: 'se', x: 1, y: 1, cursor: 'nwse-resize' },
  { h: 's', x: 0.5, y: 1, cursor: 'ns-resize' },
  { h: 'sw', x: 0, y: 1, cursor: 'nesw-resize' },
  { h: 'w', x: 0, y: 0.5, cursor: 'ew-resize' },
]

export function CropOverlay({ rect, ratio, imgAspect, onLive, onCommit }: { rect: Rect; ratio: number | null; imgAspect: number; onLive: (r: Rect) => void; onCommit: () => void }) {
  const root = useRef<HTMLDivElement>(null)
  const [x, y, w, h] = rect
  const start = (e: RPointerEvent<Element>, handle: CropHandle) => {
    if (!root.current) return
    const base = rect
    beginDrag(e, root.current, (_nx, _ny, dx, dy) => onLive(dragCropRect(base, handle, dx, dy, ratio, imgAspect)), onCommit)
  }
  return (
    <div ref={root} className="absolute inset-0" data-testid="crop-overlay">
      <div
        className="pointer-events-auto absolute cursor-move border border-white/90"
        style={{ left: `${x * 100}%`, top: `${y * 100}%`, width: `${w * 100}%`, height: `${h * 100}%`, boxShadow: '0 0 0 9999px rgba(0,0,0,0.55)' }}
        onPointerDown={(e) => start(e, 'move')}
        onDoubleClick={stop}
        data-testid="crop-rect"
      >
        {[1 / 3, 2 / 3].map((f) => (
          <span key={`v${f}`} className="pointer-events-none absolute top-0 bottom-0 w-px bg-white/40" style={{ left: `${f * 100}%` }} />
        ))}
        {[1 / 3, 2 / 3].map((f) => (
          <span key={`h${f}`} className="pointer-events-none absolute right-0 left-0 h-px bg-white/40" style={{ top: `${f * 100}%` }} />
        ))}
        {HANDLES.map((hd) => (
          <span
            key={hd.h}
            className="pointer-events-auto absolute h-3 w-3 -translate-x-1/2 -translate-y-1/2 rounded-[2px] border border-black/60 bg-white"
            style={{ left: `${hd.x * 100}%`, top: `${hd.y * 100}%`, cursor: hd.cursor }}
            onPointerDown={(e) => start(e, hd.h)}
            onDoubleClick={stop}
            data-testid={`crop-handle-${hd.h}`}
          />
        ))}
      </div>
    </div>
  )
}

// ---------------------------------------------------------------- local mask handles

const dot = 'pointer-events-auto absolute h-3.5 w-3.5 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-white bg-ai shadow'

export function MaskHandles({ index, op }: { index: number; op: LocalOp }) {
  const qc = useQueryClient()
  const { t } = useTranslation()
  const root = useRef<HTMLDivElement>(null)
  const setMask = (mask: MaskRef) => setLive(updateLocal(useEdit.getState().stack, index, (o) => ({ ...o, mask })))
  const end = () => commitLive(qc, t('history.moveMask'), `local:${index}:mask`)
  const drag = (e: RPointerEvent<Element>, fn: (nx: number, ny: number) => void) => root.current && beginDrag(e, root.current, (nx, ny) => fn(clamp(nx, 0, 1), clamp(ny, 0, 1)), end)
  const m = op.mask

  if (m.kind === 'radial') {
    const [cx, cy] = m.center
    const [rx, ry] = m.radius
    const f = m.feather ?? 0.5
    return (
      <div ref={root} className="absolute inset-0" data-testid="mask-handles">
        <svg className="pointer-events-none absolute inset-0 h-full w-full overflow-visible" viewBox="0 0 100 100" preserveAspectRatio="none">
          <ellipse cx={cx * 100} cy={cy * 100} rx={rx * 100} ry={ry * 100} fill="none" stroke="#fff" strokeWidth="1.5" vectorEffect="non-scaling-stroke" />
          <ellipse cx={cx * 100} cy={cy * 100} rx={rx * (1 - f) * 100} ry={ry * (1 - f) * 100} fill="none" stroke="#fff" strokeOpacity="0.6" strokeDasharray="4 4" strokeWidth="1" vectorEffect="non-scaling-stroke" />
        </svg>
        <span className={dot} style={{ left: `${cx * 100}%`, top: `${cy * 100}%`, cursor: 'move' }} onPointerDown={(e) => drag(e, (nx, ny) => setMask({ ...m, center: [nx, ny] }))} onDoubleClick={stop} data-testid="radial-center" />
        <span className={dot} style={{ left: `${(cx + rx) * 100}%`, top: `${cy * 100}%`, cursor: 'ew-resize', background: '#fff' }} onPointerDown={(e) => drag(e, (nx) => setMask({ ...m, radius: [Math.max(0.03, Math.abs(nx - cx)), ry] }))} onDoubleClick={stop} data-testid="radial-rx" />
        <span className={dot} style={{ left: `${cx * 100}%`, top: `${(cy + ry) * 100}%`, cursor: 'ns-resize', background: '#fff' }} onPointerDown={(e) => drag(e, (_nx, ny) => setMask({ ...m, radius: [rx, Math.max(0.03, Math.abs(ny - cy))] }))} onDoubleClick={stop} data-testid="radial-ry" />
        <span
          className={`${dot} !h-2.5 !w-2.5`}
          style={{ left: `${(cx - rx * (1 - f)) * 100}%`, top: `${cy * 100}%`, cursor: 'ew-resize' }}
          title={t('edit.feather')}
          onPointerDown={(e) => drag(e, (nx) => setMask({ ...m, feather: clamp(1 - Math.abs(nx - cx) / Math.max(rx, 0.03), 0, 0.95) }))}
          onDoubleClick={stop}
          data-testid="radial-feather"
        />
      </div>
    )
  }
  if (m.kind === 'linear') {
    const [sx, sy] = m.start
    const [ex, ey] = m.end
    return (
      <div ref={root} className="absolute inset-0" data-testid="mask-handles">
        <svg className="pointer-events-none absolute inset-0 h-full w-full overflow-visible" viewBox="0 0 100 100" preserveAspectRatio="none">
          <line x1={sx * 100} y1={sy * 100} x2={ex * 100} y2={ey * 100} stroke="#fff" strokeWidth="1.5" vectorEffect="non-scaling-stroke" />
        </svg>
        <span className={dot} style={{ left: `${sx * 100}%`, top: `${sy * 100}%`, cursor: 'move' }} onPointerDown={(e) => drag(e, (nx, ny) => setMask({ ...m, start: [nx, ny] as Point }))} onDoubleClick={stop} data-testid="linear-start" />
        <span className={dot} style={{ left: `${ex * 100}%`, top: `${ey * 100}%`, cursor: 'move', background: '#fff' }} onPointerDown={(e) => drag(e, (nx, ny) => setMask({ ...m, end: [nx, ny] as Point }))} onDoubleClick={stop} data-testid="linear-end" />
      </div>
    )
  }
  return null
}

// ---------------------------------------------------------------- mask overlay (O)

const GRID = 192

/** Red mask overlay of the active local adjustment (AI mask PNG or generated gradient). */
export function MaskOverlay({ photoId, op, aspect, crop }: { photoId: number; op: LocalOp; aspect: number; crop: CropOp | undefined }) {
  const canvas = useRef<HTMLCanvasElement>(null)
  const m = op.mask
  const isAi = m.kind === 'ai'
  const q = useQuery({
    queryKey: qk.mask(photoId, isAi ? m.target : '', isAi ? m.person_id : undefined),
    queryFn: () => fetchMask(photoId, isAi ? m.target : '', isAi ? m.person_id : undefined),
    enabled: isAi,
    staleTime: Infinity,
    retry: false,
  })

  useEffect(() => {
    const cv = canvas.current
    if (!cv) return
    const ctx = cv.getContext('2d', { willReadFrequently: true })
    if (!ctx) return
    let alive = true
    const amount = op.amount ?? 1
    const paint = (alpha: (x: number, y: number) => number, w: number, h: number) => {
      cv.width = w
      cv.height = h
      const img = ctx.createImageData(w, h)
      for (let y = 0; y < h; y++)
        for (let x = 0; x < w; x++) {
          let a = alpha(x / w, y / h)
          if (op.invert) a = 1 - a
          const i = (y * w + x) * 4
          img.data[i] = 255
          img.data[i + 1] = 60
          img.data[i + 2] = 60
          img.data[i + 3] = Math.round(a * amount * 140)
        }
      ctx.putImageData(img, 0, 0)
    }
    if (m.kind !== 'ai') {
      const w = GRID
      const h = Math.max(8, Math.round(GRID / aspect))
      paint((nx, ny) => maskValue(m, nx, ny), w, h)
      return
    }
    if (!q.data) {
      ctx.clearRect(0, 0, cv.width, cv.height)
      return
    }
    void createImageBitmap(q.data).then((bmp) => {
      if (!alive) return
      // Crop (without straighten) maps linearly onto the mask; rotated crops are approximated by the unrotated window.
      const [rx, ry, rw, rh] = crop?.rect ?? [0, 0, 1, 1]
      const sw = Math.max(1, Math.round(bmp.width * rw))
      const sh = Math.max(1, Math.round(bmp.height * rh))
      const tmp = new OffscreenCanvas(sw, sh)
      const tctx = tmp.getContext('2d', { willReadFrequently: true })!
      tctx.drawImage(bmp, Math.round(bmp.width * rx), Math.round(bmp.height * ry), sw, sh, 0, 0, sw, sh)
      const data = tctx.getImageData(0, 0, sw, sh).data
      paint((nx, ny) => data[(Math.min(sh - 1, Math.floor(ny * sh)) * sw + Math.min(sw - 1, Math.floor(nx * sw))) * 4] / 255, sw, sh)
      bmp.close()
    })
    return () => {
      alive = false
    }
  }, [m, op.invert, op.amount, q.data, aspect, crop])

  return <canvas ref={canvas} className="pointer-events-none absolute inset-0 h-full w-full" data-testid="mask-overlay" />
}
