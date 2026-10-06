import { useVirtualizer } from '@tanstack/react-virtual'
import { Ban, Flag as FlagIcon } from 'lucide-react'
import { memo, useEffect, useRef } from 'react'
import { thumbUrl } from '@/api/client'
import type { Photo } from '@/api/types'
import { colorVar } from '@/lib/colors'
import { issueBadges } from '@/lib/ai'

const ITEM = 72
const GAP = 4

interface Props {
  photos: Photo[]
  activeId: number | null
  /** Extra highlighted ids (compare mode candidates). */
  markedIds?: number[]
  /** Show 1-based shot numbers (group view). */
  numbered?: boolean
  onPick: (id: number, e?: React.MouseEvent) => void
}

const Thumb = memo(function Thumb({ photo, index, active, marked, onPick }: { photo: Photo; index: number | null; active: boolean; marked: boolean; onPick: (id: number, e?: React.MouseEvent) => void }) {
  return (
    <button
      className="relative h-full w-full overflow-hidden rounded-[2px] bg-bg"
      style={{
        outline: active ? '2px solid var(--accent)' : marked ? '2px solid var(--faint)' : 'none',
        outlineOffset: -2,
      }}
      onClick={(e) => onPick(photo.id, e)}
      aria-label={photo.file_name}
      aria-current={active}
    >
      {photo.thumb_ready ? (
        <img
          src={thumbUrl(photo, 256)}
          alt=""
          loading="lazy"
          draggable={false}
          className={`h-full w-full object-cover ${photo.flag === -1 ? 'opacity-40' : ''}`}
        />
      ) : (
        <div className="skeleton h-full w-full" />
      )}
      <div className={`absolute left-0.5 flex gap-0.5 ${index !== null ? 'top-4' : 'top-0.5'}`}>
        {photo.flag === 1 && <FlagIcon size={11} className="text-success" fill="currentColor" />}
        {photo.flag === -1 && <Ban size={11} className="text-danger" />}
      </div>
      {photo.color_label && (
        <span className="absolute top-0.5 right-0.5 h-2 w-2 rounded-full" style={{ background: colorVar(photo.color_label) }} />
      )}
      {photo.user_rating ? (
        <span className="absolute bottom-0 left-0 rounded-tr bg-black/60 px-1 text-[10px] text-accent">★{photo.user_rating}</span>
      ) : photo.ai_rating !== null ? (
        <span className="absolute bottom-0 left-0 rounded-tr bg-black/60 px-1 text-[10px] text-ai">☆{photo.ai_rating.toFixed(1)}</span>
      ) : null}
      {issueBadges(photo.issues).length > 0 && (
        <span className="absolute right-0 bottom-0 rounded-tl bg-black/60 px-0.5 text-[10px]">{issueBadges(photo.issues).slice(0, 2).map((b) => b.glyph).join('')}</span>
      )}
      {index !== null && (
        <span className="absolute top-0 left-0 rounded-br bg-black/60 px-1 text-[10px] text-white/90">
          #{index}
          {photo.rank_in_burst === 0 && <span className="ml-0.5 text-ai">✓</span>}
        </span>
      )}
    </button>
  )
})

/** Horizontal virtualized thumbnail strip; keeps the active photo in view. */
export function Filmstrip({ photos, activeId, markedIds, numbered, onPick }: Props) {
  const scrollRef = useRef<HTMLDivElement>(null)
  const virt = useVirtualizer({
    horizontal: true,
    count: photos.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ITEM + GAP,
    overscan: 10,
  })
  const activeIndex = activeId === null ? -1 : photos.findIndex((p) => p.id === activeId)
  useEffect(() => {
    if (activeIndex >= 0) virt.scrollToIndex(activeIndex, { align: 'auto' })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeIndex])

  return (
    <div ref={scrollRef} className="h-[88px] shrink-0 overflow-x-auto overflow-y-hidden border-t border-line bg-panel" data-testid="filmstrip">
      <div className="relative h-full" style={{ width: virt.getTotalSize() }}>
        {virt.getVirtualItems().map((v) => {
          const p = photos[v.index]
          return (
            <div key={p.id} className="absolute top-2" style={{ left: v.start + GAP, width: ITEM, height: ITEM }}>
              <Thumb photo={p} index={numbered ? v.index + 1 : null} active={p.id === activeId} marked={!!markedIds?.includes(p.id)} onPick={onPick} />
            </div>
          )
        })}
      </div>
    </div>
  )
}
