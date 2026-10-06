import { ChevronLeft, ChevronRight } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { previewUrl } from '@/api/client'
import type { Photo } from '@/api/types'
import { useKeyHeld } from '@/lib/hooks'
import { FIT_VIEW, type ViewState } from '@/lib/zoom'
import { useUi } from '@/stores/ui'
import { ZoomPane } from './ZoomPane'
import { Filmstrip } from './Filmstrip'
import { AiRatingSlot, StarRating } from './controls'

interface Props {
  photos: Photo[]
  photo: Photo | undefined
  index: number
  onMove: (delta: number) => void
  onPick: (id: number) => void
  onRate: (n: number | null) => void
}

const PRELOAD = 2

/** Single-photo view: fit / 100% / wheel zoom, pan, progressive loading, neighbour preloading, filmstrip. */
export function Loupe({ photos, photo, index, onMove, onPick, onRate }: Props) {
  const { t } = useTranslation()
  // View is keyed by photo id so it resets to "fit" automatically when the photo changes.
  const [state, setState] = useState<{ id: number | null; view: ViewState }>({ id: null, view: FIT_VIEW })
  const [hover, setHover] = useState({ nx: 0.5, ny: 0.5 })
  const toggleSignal = useUi((s) => s.zoomToggle)
  const zHeld = useKeyHeld('z')

  const id = photo?.id ?? null
  const view = state.id === id ? state.view : FIT_VIEW

  // Preload neighbours' 2048 previews so ←/→ feels instant.
  useEffect(() => {
    const imgs: HTMLImageElement[] = []
    for (let d = -PRELOAD; d <= PRELOAD; d++) {
      const p = photos[index + d]
      if (p && d !== 0) {
        const im = new Image()
        im.src = previewUrl(p.id, 2048)
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
              <AiRatingSlot value={photo.ai_rating} />
            </div>
          </div>
        </div>
      </div>
      <Filmstrip photos={photos} activeId={photo.id} onPick={onPick} />
    </div>
  )
}
