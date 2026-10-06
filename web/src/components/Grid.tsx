import { useVirtualizer } from '@tanstack/react-virtual'
import { Ban, Flag as FlagIcon, ImageOff } from 'lucide-react'
import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { api, thumbUrl } from '@/api/client'
import type { Photo } from '@/api/types'
import { useDebouncedEffect, useElementSize } from '@/lib/hooks'
import { clickSelect } from '@/lib/selection'
import { useUi } from '@/stores/ui'
import { colorVar } from '@/lib/colors'

const GAP = 4
const PAD = 8

interface CellProps {
  photo: Photo
  size: number
  selected: boolean
  active: boolean
}

const Cell = memo(function Cell({ photo, size, selected, active }: CellProps) {
  const [failed, setFailed] = useState(false)
  const showBadges = size >= 100
  const rejected = photo.flag === -1
  return (
    <div
      data-id={photo.id}
      role="gridcell"
      aria-selected={selected}
      className="relative cursor-pointer overflow-hidden rounded-[2px] bg-panel"
      style={{
        width: size,
        height: size,
        boxShadow: selected
          ? 'inset 0 0 0 3px var(--accent)'
          : active
            ? 'inset 0 0 0 2px var(--fg)'
            : undefined,
      }}
    >
      {photo.thumb_ready && !failed ? (
        <img
          src={thumbUrl(photo, 256)}
          alt={photo.file_name}
          loading="lazy"
          decoding="async"
          draggable={false}
          onError={() => setFailed(true)}
          className="h-full w-full object-contain transition-opacity"
          style={{ opacity: rejected ? 0.4 : 1 }}
        />
      ) : failed ? (
        <div className="flex h-full w-full items-center justify-center text-faint">
          <ImageOff size={20} />
        </div>
      ) : (
        <div className="skeleton h-full w-full" />
      )}
      {selected && <div className="pointer-events-none absolute inset-0 bg-accent/10" />}
      <div className="pointer-events-none absolute top-1 left-1 flex items-center gap-1">
        {photo.flag === 1 && <FlagIcon size={14} className="text-success drop-shadow" fill="currentColor" />}
        {rejected && <Ban size={14} className="text-danger drop-shadow" />}
      </div>
      {photo.color_label && (
        <span
          className="pointer-events-none absolute top-1 right-1 h-2.5 w-2.5 rounded-full ring-1 ring-black/50"
          style={{ background: colorVar(photo.color_label) }}
        />
      )}
      {showBadges && (
        <>
          {photo.user_rating ? (
            <span className="tnum pointer-events-none absolute bottom-1 left-1 rounded bg-black/65 px-1 text-[11px] leading-4 text-accent">
              {'★'.repeat(photo.user_rating)}
            </span>
          ) : null}
          {photo.format !== 'jpeg' && (
            <span
              className={`pointer-events-none absolute right-1 bottom-1 rounded px-1 text-[10px] leading-4 font-semibold uppercase ${
                photo.format === 'raw' ? 'bg-accent text-accent-fg' : 'bg-black/65 text-white/85'
              }`}
            >
              {photo.format}
            </span>
          )}
        </>
      )}
    </div>
  )
})

interface Props {
  photos: Photo[]
  onOpen: (id: number) => void
}

/** Virtualized thumbnail grid: only visible rows are mounted (20k+ items). */
export function Grid({ photos, onOpen }: Props) {
  const { t } = useTranslation()
  const [ref, size] = useElementSize<HTMLDivElement>()
  const thumbSize = useUi((s) => s.thumbSize)
  const selection = useUi((s) => s.selection)
  const activeId = useUi((s) => s.activeId)

  const innerW = Math.max(0, size.w - PAD * 2)
  const cols = Math.max(1, Math.floor((innerW + GAP) / (thumbSize + GAP)))
  const cell = Math.max(32, Math.floor((innerW - GAP * (cols - 1)) / cols))
  const rowCount = Math.ceil(photos.length / cols)
  const setGridCols = useUi((s) => s.setGridCols)
  useEffect(() => setGridCols(cols), [cols, setGridCols])

  const virt = useVirtualizer({
    count: rowCount,
    getScrollElement: () => ref.current,
    estimateSize: () => cell + GAP,
    overscan: 4,
    paddingStart: PAD,
    paddingEnd: PAD,
  })

  // Re-measure when the cell size changes.
  useEffect(() => {
    virt.measure()
  }, [cell, virt])

  // Keep the active cell visible.
  const activeIndex = useMemo(() => (activeId === null ? -1 : photos.findIndex((p) => p.id === activeId)), [photos, activeId])
  useEffect(() => {
    if (activeIndex >= 0 && cols > 0) virt.scrollToIndex(Math.floor(activeIndex / cols), { align: 'auto' })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeIndex, cols])

  // Tell the server which thumbnails are on screen (debounced) so it can prioritize them.
  const items = virt.getVirtualItems()
  const first = items[0]?.index ?? 0
  const last = items[items.length - 1]?.index ?? 0
  const photosRef = useRef(photos)
  useEffect(() => {
    photosRef.current = photos
  })
  useDebouncedEffect(
    () => {
      const ids: number[] = []
      const end = Math.min(photosRef.current.length, (last + 1) * cols)
      for (let i = first * cols; i < end; i++) {
        const p = photosRef.current[i]
        if (!p.thumb_ready) ids.push(p.id)
      }
      if (ids.length) void api.viewport(ids.slice(0, 500)).catch(() => undefined)
    },
    [first, last, cols, photos.length],
    150,
  )

  const idOrder = useMemo(() => photos.map((p) => p.id), [photos])
  const idFromEvent = (e: React.MouseEvent): number | null => {
    const el = (e.target as HTMLElement).closest('[data-id]')
    return el ? Number(el.getAttribute('data-id')) : null
  }
  const onClick = useCallback(
    (e: React.MouseEvent) => {
      const id = idFromEvent(e)
      const ui = useUi.getState()
      if (id === null) {
        ui.setSelection({ ids: new Set(), anchorId: null })
        return
      }
      ui.setActive(id)
      ui.setSelection(clickSelect(ui.selection, idOrder, id, { shift: e.shiftKey, ctrl: e.ctrlKey || e.metaKey }))
    },
    [idOrder],
  )
  const onDoubleClick = (e: React.MouseEvent) => {
    const id = idFromEvent(e)
    if (id !== null) onOpen(id)
  }

  return (
    <div
      ref={ref}
      className="h-full overflow-y-auto"
      onClick={onClick}
      onDoubleClick={onDoubleClick}
      role="grid"
      aria-label={t('grid.label')}
      aria-rowcount={rowCount}
      data-testid="grid"
    >
      {photos.length === 0 ? (
        <div className="flex h-full items-center justify-center text-muted">{t('grid.empty')}</div>
      ) : (
        <div className="relative w-full" style={{ height: virt.getTotalSize() }}>
          {items.map((row) => {
            const start = row.index * cols
            const slice = photos.slice(start, start + cols)
            return (
              <div
                key={row.index}
                role="row"
                className="absolute top-0 left-0 flex"
                style={{ transform: `translateY(${row.start}px)`, gap: GAP, paddingLeft: PAD, height: cell }}
              >
                {slice.map((p) => (
                  <Cell key={p.id} photo={p} size={cell} selected={selection.ids.has(p.id)} active={p.id === activeId} />
                ))}
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}
