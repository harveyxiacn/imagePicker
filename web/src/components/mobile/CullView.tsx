import { useQueryClient } from '@tanstack/react-query'
import { Check, ChevronLeft, Layers, Pencil, Star, X } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { previewUrl, thumbUrl } from '@/api/client'
import type { Flag, Photo } from '@/api/types'
import { editPhotos, undo } from '@/lib/actions'
import { haptic } from '@/lib/haptics'
import { swipeProgress, liveDirection, type SwipeDir } from '@/lib/gestures'
import { useElementSize } from '@/lib/hooks'
import { useToasts } from '@/stores/toasts'
import { AiStars, StarRating } from '../controls'
import { FaceBoxes } from '../FaceBoxes'
import { useGestureSurface } from './useGestureSurface'

interface Props {
  photos: Photo[]
  photo: Photo | undefined
  index: number
  onMove: (delta: number) => void
  onFlag: (f: Flag) => void
  onRate: (n: number | null) => void
  onEdit?: () => void
  onBack: () => void
  onInfo: () => void
  onQuick: () => void
  showFaces?: boolean
}

const EXIT_MS = 190

/**
 * Mobile cull view (doc 08 section 5): one full-screen photo.
 * Swipe left/right = next/previous, up = pick, down = reject (animated, with an undo snackbar),
 * double-tap / pinch = zoom, long-press = multi-select, star bar = rating.
 */
export function CullView({ photos, photo, index, onMove, onFlag, onRate, onEdit, onBack, onInfo, onQuick, showFaces }: Props) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const [boxRef, box] = useElementSize<HTMLDivElement>()
  const [selected, setSelected] = useState<ReadonlySet<number>>(new Set())
  const [exit, setExit] = useState<SwipeDir | null>(null)
  const [loaded, setLoaded] = useState<number | null>(null)
  const selecting = selected.size > 0
  const id = photo?.id ?? null

  const latest = useRef({ photo, index, photos, selecting })
  useEffect(() => {
    latest.current = { photo, index, photos, selecting }
  })

  // Preload neighbours so a swipe lands on a decoded image.
  useEffect(() => {
    const imgs: HTMLImageElement[] = []
    for (const d of [1, 2, -1]) {
      const p = photos[index + d]
      if (!p) continue
      const im = new Image()
      im.src = previewUrl(p.id, 2048, p.thumb_version)
      imgs.push(im)
    }
    return () => {
      for (const im of imgs) im.onload = null
    }
  }, [photos, index])

  const snack = (label: string, flagged: Flag) => {
    useToasts.getState().push(flagged === 1 ? 'success' : 'info', label, 4500, [
      {
        label: t('mobile.cull.undo'),
        onClick: () => {
          void undo(qc)
          haptic('tick')
        },
      },
    ])
  }

  const g = useGestureSurface({
    box,
    resetKey: id,
    zoomable: true,
    onSwipe: (dir) => {
      const { photo: p, index: i, photos: list, selecting: sel } = latest.current
      if (!p) return false
      if ((dir === 'up' || dir === 'down') && sel) return false
      if (dir === 'left' || dir === 'right') {
        const delta = dir === 'left' ? 1 : -1
        if (i + delta < 0 || i + delta >= list.length) {
          haptic('tick')
          return false
        }
      }
      haptic(dir === 'down' ? 'reject' : dir === 'up' ? 'pick' : 'tick')
      setExit(dir)
      setTimeout(() => {
        if (dir === 'left') onMove(1)
        else if (dir === 'right') onMove(-1)
        else {
          const flag: Flag = dir === 'up' ? 1 : -1
          onFlag(flag)
          snack(t(flag === 1 ? 'mobile.cull.picked' : 'mobile.cull.rejected', { name: p.file_name }), flag)
          if (i + 1 < list.length) onMove(1)
        }
        setExit(null)
        g.resetDrag()
      }, EXIT_MS)
    },
    onTap: () => {
      const p = latest.current.photo
      if (p && latest.current.selecting) toggle(p.id)
    },
    onLongPress: () => {
      const p = latest.current.photo
      if (!p) return
      haptic('select')
      toggle(p.id)
    },
  })

  function toggle(pid: number) {
    setSelected((s) => {
      const n = new Set(s)
      if (n.has(pid)) n.delete(pid)
      else n.add(pid)
      return n
    })
  }

  const bulk = (patch: { flag: Flag }) => {
    const ids = [...selected]
    void editPhotos(qc, ids, patch, t('history.flag'))
    haptic(patch.flag === 1 ? 'pick' : 'reject')
    useToasts.getState().push('info', t(patch.flag === 1 ? 'mobile.cull.bulkPicked' : 'mobile.cull.bulkRejected', { n: ids.length }), 4500, [
      { label: t('mobile.cull.undo'), onClick: () => void undo(qc) },
    ])
    setSelected(new Set())
  }

  if (!photo) return <div className="flex h-full items-center justify-center text-muted">{t('grid.empty')}</div>

  const zoomed = g.zoom.scale > 1.02
  const dir = zoomed || selecting ? null : liveDirection(g.drag.dx, g.drag.dy)
  const up = dir ? swipeProgress(g.drag.dx, g.drag.dy, 'up') : 0
  const down = dir ? swipeProgress(g.drag.dx, g.drag.dy, 'down') : 0
  const side = dir === 'left' || dir === 'right' ? swipeProgress(g.drag.dx, g.drag.dy, dir) : 0

  // Live transform: follows the finger; on commit flies out in the swipe direction.
  const fly = exit === 'left' ? { x: -1.1, y: 0 } : exit === 'right' ? { x: 1.1, y: 0 } : exit === 'up' ? { x: 0, y: -1.1 } : exit === 'down' ? { x: 0, y: 1.1 } : null
  const tx = fly ? fly.x * box.w : zoomed ? 0 : g.drag.dx
  const ty = fly ? fly.y * box.h : zoomed ? 0 : g.drag.dy
  const rot = zoomed ? 0 : Math.max(-8, Math.min(8, (fly ? fly.x * 8 : g.drag.dx / 22)))
  const animating = exit !== null || (g.drag.dx === 0 && g.drag.dy === 0)

  const flagTone = photo.flag === 1 ? 'bg-success text-black' : photo.flag === -1 ? 'bg-danger text-white' : ''

  return (
    <div className="relative flex h-full min-h-0 flex-col bg-black" data-testid="cull-view" data-photo-id={photo.id}>
      <div ref={boxRef} className="relative min-h-0 flex-1 touch-none overflow-hidden select-none" {...g.bind} data-testid="cull-surface">
        <div
          className="absolute inset-0"
          style={{
            transform: `translate(${tx}px, ${ty}px) rotate(${rot}deg)`,
            transition: animating ? `transform ${EXIT_MS}ms var(--ease), opacity ${EXIT_MS}ms` : 'none',
            opacity: fly ? 0.15 : 1,
          }}
          data-testid="cull-card"
        >
          <div
            className="absolute inset-0"
            style={{
              transform: `translate(${g.zoom.tx}px, ${g.zoom.ty}px) scale(${g.zoom.scale})`,
              transition: 'transform 120ms ease-out',
              willChange: 'transform',
            }}
          >
            <img src={thumbUrl(photo, 256)} alt="" draggable={false} className="pointer-events-none absolute inset-0 h-full w-full object-contain" />
            <img
              key={photo.id}
              src={previewUrl(photo.id, 2048, photo.thumb_version)}
              alt={photo.file_name}
              draggable={false}
              onLoad={() => setLoaded(photo.id)}
              className="pointer-events-none absolute inset-0 h-full w-full object-contain transition-opacity duration-150"
              style={{ opacity: loaded === photo.id ? 1 : 0 }}
              data-testid="cull-image"
            />
            {showFaces && photo.analyzed && <FaceBoxes photoId={photo.id} />}
          </div>
          {selected.has(photo.id) && (
            <div className="absolute inset-0 border-4 border-accent">
              <span className="absolute top-14 left-3 flex h-7 w-7 items-center justify-center rounded-full bg-accent text-accent-fg">
                <Check size={16} />
              </span>
            </div>
          )}
        </div>

        {/* swipe feedback */}
        <div className="pointer-events-none absolute inset-x-0 top-1/4 flex justify-center" style={{ opacity: up, transform: `scale(${0.7 + up * 0.4})` }} data-testid="cull-hint-up">
          <span className="flex items-center gap-2 rounded-full bg-success px-5 py-2 text-lg font-bold text-black shadow-lg">
            <Star size={20} fill="currentColor" />
            {t('mobile.cull.pick')}
          </span>
        </div>
        <div className="pointer-events-none absolute inset-x-0 bottom-1/4 flex justify-center" style={{ opacity: down, transform: `scale(${0.7 + down * 0.4})` }} data-testid="cull-hint-down">
          <span className="flex items-center gap-2 rounded-full bg-danger px-5 py-2 text-lg font-bold text-white shadow-lg">
            <X size={20} />
            {t('mobile.cull.reject')}
          </span>
        </div>
        {(dir === 'left' || dir === 'right') && (
          <div className={`pointer-events-none absolute top-1/2 -translate-y-1/2 rounded-full bg-black/60 p-3 text-white ${dir === 'left' ? 'right-3' : 'left-3'}`} style={{ opacity: side }}>
            <ChevronLeft size={24} className={dir === 'left' ? 'rotate-180' : ''} />
          </div>
        )}

        {/* top bar */}
        <div
          className="pointer-events-none absolute inset-x-0 top-0 flex items-center gap-2 bg-gradient-to-b from-black/70 to-transparent px-2 pt-2 pb-6 text-white"
        >
          {selecting ? (
            <div className="pointer-events-auto flex w-full items-center gap-2" data-testid="cull-selection-bar">
              <button className="btn btn-icon bg-black/50" onClick={() => setSelected(new Set())} aria-label={t('common.cancel')}>
                <X size={18} />
              </button>
              <span className="tnum flex-1 text-base font-semibold" data-testid="cull-selection-count">
                {t('mobile.cull.selected', { n: selected.size })}
              </span>
              <button className="btn bg-success/90 text-black" onClick={() => bulk({ flag: 1 })} data-testid="cull-bulk-pick">
                <Star size={15} />
                {t('mobile.cull.pick')}
              </button>
              <button className="btn bg-danger/90 text-white" onClick={() => bulk({ flag: -1 })} data-testid="cull-bulk-reject">
                <X size={15} />
                {t('mobile.cull.reject')}
              </button>
            </div>
          ) : (
            <>
              <button className="btn btn-icon pointer-events-auto bg-black/50" onClick={onBack} aria-label={t('common.back')} data-testid="cull-back">
                <ChevronLeft size={20} />
              </button>
              <div className="min-w-0 flex-1">
                <div className="tnum truncate text-sm font-medium">{photo.file_name}</div>
                <div className="tnum text-xs text-white/70" data-testid="cull-counter">
                  {index + 1} / {photos.length}
                </div>
              </div>
              {flagTone && <span className={`rounded-full px-2.5 py-1 text-xs font-bold ${flagTone}`} data-testid="cull-flag">{t(photo.flag === 1 ? 'mobile.cull.pick' : 'mobile.cull.reject')}</span>}
              <button className="btn pointer-events-auto bg-black/50 text-white" onClick={onQuick} data-testid="quick-cull-toggle">
                <Layers size={16} />
                {t('mobile.cull.quick')}
              </button>
              {onEdit && (
                <button className="btn btn-icon pointer-events-auto bg-black/50" onClick={onEdit} aria-label={t('edit.open')} data-testid="open-edit">
                  <Pencil size={16} />
                </button>
              )}
            </>
          )}
        </div>

        {zoomed && (
          <button className="btn pointer-events-auto absolute bottom-3 left-1/2 -translate-x-1/2 bg-black/60 text-white" onClick={g.resetZoom} onPointerDown={(e) => e.stopPropagation()}>
            {t('loupe.fit')}
          </button>
        )}
      </div>

      {/* bottom: rating + info handle (swipe up / tap opens the inspector sheet) */}
      <div className="shrink-0 border-t border-line bg-panel" data-testid="cull-bottom">
        <InfoHandle onOpen={onInfo} label={t('mobile.cull.info')} />
        <div className="flex items-center justify-between gap-2 px-3 pb-2">
          <div className="cull-stars">
            <StarRating value={photo.user_rating} onChange={onRate} size={26} />
          </div>
          <AiStars value={photo.ai_rating} />
        </div>
        <div className="px-3 pb-2 text-center text-[11px] text-faint">{t('mobile.cull.gestureHint')}</div>
      </div>
    </div>
  )
}

/** Grab handle: a tap or an upward swipe opens the inspector sheet. */
function InfoHandle({ onOpen, label }: { onOpen: () => void; label: string }) {
  const y0 = useRef<number | null>(null)
  return (
    <button
      className="flex w-full touch-none flex-col items-center py-2"
      aria-label={label}
      data-testid="cull-info-handle"
      onPointerDown={(e) => {
        y0.current = e.clientY
        e.currentTarget.setPointerCapture(e.pointerId)
      }}
      onPointerUp={(e) => {
        const start = y0.current
        y0.current = null
        if (start === null) return
        if (start - e.clientY > 24 || Math.abs(e.clientY - start) < 8) onOpen()
      }}
    >
      <span className="h-1.5 w-10 rounded-full bg-faint/60" />
    </button>
  )
}
