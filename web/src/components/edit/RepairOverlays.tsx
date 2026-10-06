import { useEffect, useRef, useState, type PointerEvent as RPointerEvent } from 'react'
import { useTranslation } from 'react-i18next'
import { useBystanders } from '@/api/queries'
import type { CropOp, Point } from '@/api/types'
import { appendPoint, finishStroke, frameToSource, radiusToDisplay, rectToFrame, sourceToFrame } from '@/lib/strokes'
import { useRepair } from '@/stores/repair'

/** Outlines of the detected bystanders while hovering "消除路人" (source coordinates mapped through crop / straighten). */
export function BystanderOverlay({ photoId, crop, aspect }: { photoId: number; crop: CropOp | undefined; aspect: number }) {
  const { t } = useTranslation()
  const faces = useBystanders(photoId).data ?? []
  return (
    <div className="pointer-events-none absolute inset-0" data-testid="bystander-overlay">
      {faces.map((f) => {
        const [x, y, w, h] = f.bbox
        const [u0, v0] = sourceToFrame(x, y, crop, aspect)
        const [u1, v1] = sourceToFrame(x + w, y + h, crop, aspect)
        const pad = 0.012
        return (
          <div
            key={f.face_id}
            className="absolute rounded-sm border-2 border-dashed border-danger"
            style={{
              left: `${(u0 - pad) * 100}%`,
              top: `${(v0 - pad) * 100}%`,
              width: `${(u1 - u0 + pad * 2) * 100}%`,
              height: `${(v1 - v0 + pad * 2) * 100}%`,
              background: 'color-mix(in srgb, var(--danger) 18%, transparent)',
              boxShadow: '0 0 0 1px rgba(0,0,0,.4)',
            }}
            data-testid="bystander-box"
          >
            <span className="absolute -top-5 left-0 rounded bg-danger px-1 text-[10px] leading-4 whitespace-nowrap text-white">{t('repair.bystander')}</span>
          </div>
        )
      })}
    </div>
  )
}

/**
 * Brush erase tool: paints strokes (red preview) in normalised SOURCE coordinates. The overlay root has exactly the
 * on-screen size of the rendered frame, so pointer positions are measured from its DOM rectangle (zoom and pan already
 * applied) and then mapped through crop / straighten (`lib/strokes`). Space or middle button drag pans.
 */
export function BrushOverlay({ crop, aspect }: { crop: CropOp | undefined; aspect: number }) {
  const root = useRef<HTMLDivElement>(null)
  const [box, setBox] = useState({ w: 0, h: 0 })
  const radius = useRepair((s) => s.radius)
  const strokes = useRepair((s) => s.strokes)
  const [live, setLive] = useState<Point[]>([])
  const [cursor, setCursor] = useState<{ x: number; y: number } | null>(null)
  const space = useRef(false)

  // measure the frame rectangle (the overlay is positioned over it)
  useEffect(() => {
    const el = root.current
    if (!el) return
    const ro = new ResizeObserver(([e]) => setBox({ w: e.contentRect.width, h: e.contentRect.height }))
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  useEffect(() => {
    const down = (e: KeyboardEvent) => {
      if (e.key === ' ') space.current = true
    }
    const up = (e: KeyboardEvent) => {
      if (e.key === ' ') space.current = false
    }
    window.addEventListener('keydown', down)
    window.addEventListener('keyup', up)
    return () => {
      window.removeEventListener('keydown', down)
      window.removeEventListener('keyup', up)
    }
  }, [])

  const toSource = (e: { clientX: number; clientY: number }): Point => {
    const r = root.current!.getBoundingClientRect()
    const [u, v] = rectToFrame(r, e.clientX, e.clientY)
    return frameToSource(u, v, crop, aspect)
  }
  const local = (e: { clientX: number; clientY: number }) => {
    const r = root.current!.getBoundingClientRect()
    return { x: e.clientX - r.left, y: e.clientY - r.top }
  }

  const down = (e: RPointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 || space.current) return // let the pane pan
    e.stopPropagation()
    e.preventDefault()
    e.currentTarget.setPointerCapture(e.pointerId)
    setLive([toSource(e)])
  }
  const move = (e: RPointerEvent<HTMLDivElement>) => {
    setCursor(local(e))
    if (live.length === 0) return
    setLive((pts) => appendPoint(pts, toSource(e), radius, aspect))
  }
  const up = () => {
    if (live.length === 0) return
    const s = finishStroke(live, radius)
    if (s) useRepair.getState().addStroke(s)
    setLive([])
  }

  const px = (p: Point): [number, number] => {
    const [u, v] = sourceToFrame(p[0], p[1], crop, aspect)
    return [u * box.w, v * box.h]
  }
  const draw = (pts: Point[], r: number, key: string) => {
    const rr = radiusToDisplay(r, crop, box.w)
    if (pts.length === 1) {
      const [x, y] = px(pts[0])
      return <circle key={key} cx={x} cy={y} r={rr} fill="rgba(255,40,40,0.5)" />
    }
    return <polyline key={key} points={pts.map((p) => px(p).join(',')).join(' ')} fill="none" stroke="rgba(255,40,40,0.5)" strokeWidth={rr * 2} strokeLinecap="round" strokeLinejoin="round" />
  }

  return (
    <div
      ref={root}
      className="pointer-events-auto absolute inset-0 touch-none"
      style={{ cursor: 'none' }}
      onPointerDown={down}
      onPointerMove={move}
      onPointerUp={up}
      onPointerCancel={up}
      onPointerLeave={() => setCursor(null)}
      onDoubleClick={(e) => e.stopPropagation()}
      data-testid="brush-overlay"
    >
      <svg className="pointer-events-none absolute inset-0 h-full w-full" viewBox={`0 0 ${Math.max(1, box.w)} ${Math.max(1, box.h)}`} data-testid="brush-strokes" data-count={strokes.length}>
        {strokes.map((s, i) => draw(s.points, s.radius, `s${i}`))}
        {live.length > 0 && draw(live, radius, 'live')}
      </svg>
      {cursor && (
        <span
          className="pointer-events-none absolute rounded-full border border-white/90"
          style={{
            left: cursor.x,
            top: cursor.y,
            width: radiusToDisplay(radius, crop, box.w) * 2,
            height: radiusToDisplay(radius, crop, box.w) * 2,
            transform: 'translate(-50%, -50%)',
            boxShadow: '0 0 0 1px rgba(0,0,0,.6)',
          }}
        />
      )}
    </div>
  )
}
