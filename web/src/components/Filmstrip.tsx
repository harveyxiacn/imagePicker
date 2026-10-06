import { useVirtualizer } from '@tanstack/react-virtual'
import { Ban, Flag as FlagIcon } from 'lucide-react'
import { memo, useEffect, useRef } from 'react'
import { thumbUrl } from '@/api/client'
import type { Photo } from '@/api/types'
import { colorVar } from '@/lib/colors'

const ITEM = 72
const GAP = 4

interface Props {
  photos: Photo[]
  activeId: number | null
  /** Extra highlighted ids (compare mode candidates). */
  markedIds?: number[]
  onPick: (id: number) => void
}

const Thumb = memo(function Thumb({ photo, active, marked, onPick }: { photo: Photo; active: boolean; marked: boolean; onPick: (id: number) => void }) {
  return (
    <button
      className="relative h-full w-full overflow-hidden rounded-[2px] bg-bg"
      style={{
        outline: active ? '2px solid var(--accent)' : marked ? '2px solid var(--faint)' : 'none',
        outlineOffset: -2,
      }}
      onClick={() => onPick(photo.id)}
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
      <div className="absolute top-0.5 left-0.5 flex gap-0.5">
        {photo.flag === 1 && <FlagIcon size={11} className="text-success" fill="currentColor" />}
        {photo.flag === -1 && <Ban size={11} className="text-danger" />}
      </div>
      {photo.color_label && (
        <span className="absolute top-0.5 right-0.5 h-2 w-2 rounded-full" style={{ background: colorVar(photo.color_label) }} />
      )}
      {photo.user_rating ? (
        <span className="absolute bottom-0 left-0 rounded-tr bg-black/60 px-1 text-[10px] text-accent">★{photo.user_rating}</span>
      ) : null}
    </button>
  )
})

/** Horizontal virtualized thumbnail strip; keeps the active photo in view. */
export function Filmstrip({ photos, activeId, markedIds, onPick }: Props) {
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
              <Thumb photo={p} active={p.id === activeId} marked={!!markedIds?.includes(p.id)} onPick={onPick} />
            </div>
          )
        })}
      </div>
    </div>
  )
}
