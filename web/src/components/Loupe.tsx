import { ChevronLeft, ChevronRight, Pencil } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { previewUrl } from '@/api/client'
import type { Photo } from '@/api/types'
import { useKeyHeld } from '@/lib/hooks'
import { FIT_VIEW, type ViewState } from '@/lib/zoom'
import { useUi } from '@/stores/ui'
import { FaceBoxes } from './FaceBoxes'
import { ZoomPane } from './ZoomPane'
import { Filmstrip } from './Filmstrip'
import { AiStars, StarRating } from './controls'

interface Props {
  photos: Photo[]
  photo: Photo | undefined
  index: number
  onMove: (delta: number) => void
  onPick: (id: number) => void
  onRate: (n: number | null) => void
  onEdit?: () => void
}

/** Neighbours preloaded (and decoded) ahead of / behind the current photo. */
const PRELOAD_AHEAD = 3
const PRELOAD_BEHIND = 2

/** Single-photo view: fit / 100% / wheel zoom, pan, progressive loading, neighbour preloading, filmstrip. */
export function Loupe({ photos, photo, index, onMove, onPick, onRate, onEdit }: Props) {
  const { t } = useTranslation()
  // View is keyed by photo id so it resets to "fit" automatically when the photo changes.
  const [state, setState] = useState<{ id: number | null; view: ViewState }>({ id: null, view: FIT_VIEW })
  const [hover, setHover] = useState({ nx: 0.5, ny: 0.5 })
  const toggleSignal = useUi((s) => s.zoomToggle)
  const zHeld = useKeyHeld('z')
  const showFaces = useUi((s) => s.showFaces)

  const id = photo?.id ?? null
  const view = state.id === id ? state.view : FIT_VIEW

  // Preload neighbours' 2048 previews so ←/→ feels instant: the nearest first, and decoded
  // (`decode()`), so the first paint after a key press does not wait for the JPEG decode.
  useEffect(() => {
    const imgs: HTMLImageElement[] = []
    for (let d = -PRELOAD_BEHIND; d <= PRELOAD_AHEAD; d++) {
      const p = photos[index + d]
      if (p && d !== 0) {
        const im = new Image()
        im.fetchPriority = Math.abs(d) <= 1 ? 'high' : 'low'
        im.src = previewUrl(p.id, 2048, p.thumb_version)
        void im.decode().catch(() => undefined)
        imgs.push(im)
      }
    }
    return () => {
      for (const im of imgs) im.onload = null
    }
  }, [photos, index])

  if (!photo) {
    return <div className="flex h-full items-center justify-center text-muted">{t('grid.empty')}</div>
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="relative min-h-0 flex-1">
        <ZoomPane
          photo={photo}
          view={view}
          onViewChange={(v) => setState({ id, view: v })}
          toggleSignal={toggleSignal}
          hold={zHeld ? hover : null}
          onHover={(nx, ny) => setHover({ nx, ny })}
          onSwipe={(d) => onMove(d)}
          overlay={showFaces && photo.analyzed ? <FaceBoxes photoId={photo.id} /> : undefined}
        />
        <button
          className="btn btn-icon absolute top-1/2 left-2 -translate-y-1/2 bg-black/50 backdrop-blur disabled:hidden"
          aria-label={t('keys.prev')}
          disabled={index <= 0}
          onClick={() => onMove(-1)}
        >
          <ChevronLeft size={16} />
        </button>
        <button
          className="btn btn-icon absolute top-1/2 right-2 -translate-y-1/2 bg-black/50 backdrop-blur disabled:hidden"
          aria-label={t('keys.next')}
          disabled={index >= photos.length - 1}
          onClick={() => onMove(1)}
        >
          <ChevronRight size={16} />
        </button>
        <div className="pointer-events-none absolute right-0 bottom-0 left-0 flex items-end justify-between bg-gradient-to-t from-black/70 to-transparent p-3 text-white">
          <div className="pointer-events-auto flex flex-col gap-1">
            <div className="tnum text-sm font-medium">
              {photo.file_name}
              <span className="ml-2 text-xs text-white/60">
                {index + 1} / {photos.length}
              </span>
            </div>
            <div className="flex items-center gap-3">
              <StarRating value={photo.user_rating} onChange={onRate} size={15} />
              <AiStars value={photo.ai_rating} />
            </div>
          </div>
          {onEdit && (
            <button className="btn pointer-events-auto bg-black/50 backdrop-blur" onClick={onEdit} data-testid="open-edit" title={`${t('edit.open')} (D)`}>
              <Pencil size={14} />
              {t('edit.open')}
            </button>
          )}
        </div>
      </div>
      <Filmstrip photos={photos} activeId={photo.id} onPick={onPick} />
    </div>
  )
}
