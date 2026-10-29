import { useVirtualizer } from '@tanstack/react-virtual'
import { Ban, ChevronDown, ChevronRight, Flag as FlagIcon, FilterX, ImageOff, Layers, Unlink } from 'lucide-react'
import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { TFunction } from 'i18next'
import { api, thumbUrl } from '@/api/client'
import type { Photo } from '@/api/types'
import { issueBadges } from '@/lib/ai'
import { formatClock } from '@/lib/format'
import { useDebouncedEffect, useElementSize } from '@/lib/hooks'
import { photoLabel } from '@/lib/photoLabel'
import { aiScoreTip } from '@/lib/score'
import { clickSelect } from '@/lib/selection'
import { buildRows, visiblePhotos, type GridItem, type HeaderItem, type StackInfo } from '@/lib/stacks'
import { useUi } from '@/stores/ui'
import { colorVar } from '@/lib/colors'

/** DOM id of a photo's cell: the grid points `aria-activedescendant` at it. */
const cellId = (photoId: number) => `grid-cell-${photoId}`

const GAP = 4
const PAD = 8
const HEADER_H = 34

interface CellProps {
  photo: Photo
  stack: StackInfo | null
  size: number
  selected: boolean
  active: boolean
  onToggleStack: (burstId: number) => void
  /** Passed down (not read via `useTranslation`) so a mounted cell carries no i18n subscription. */
  t: TFunction
}

const sameStack = (a: StackInfo | null, b: StackInfo | null) =>
  a === b || (!!a && !!b && a.burstId === b.burstId && a.count === b.count && a.expanded === b.expanded && a.isCover === b.isCover)

const Cell = memo(function Cell({ photo, stack, size, selected, active, onToggleStack, t }: CellProps) {
  const [failed, setFailed] = useState(false)
  const showBadges = size >= 100
  const rejected = photo.flag === -1
  const collapsedStack = stack !== null && !stack.expanded
  const badges = showBadges ? issueBadges(photo.issues) : []
  const missing = photo.missing === true
  const paired = (photo.companions?.length ?? 0) > 0
  return (
    <div
      className="relative shrink-0"
      style={{
        width: size,
        height: size,
        // a collapsed stack looks like a pile of prints
        boxShadow: collapsedStack ? '2px 2px 0 0 var(--faint), 4px 4px 0 0 var(--line)' : undefined,
      }}
    >
      <div
        data-id={photo.id}
        id={cellId(photo.id)}
        role="gridcell"
        aria-selected={selected}
        aria-label={photoLabel(photo, t, stack)}
        // keyboard focus lives on the grid (aria-activedescendant): ring the current cell while it is shown
        className={`relative h-full w-full cursor-pointer overflow-hidden rounded-[2px] bg-panel ${
          active ? 'group-focus-visible:[outline-offset:-3px] group-focus-visible:[outline:3px_solid_var(--fg)]' : ''
        }`}
        style={{
          boxShadow: selected
            ? 'inset 0 0 0 3px var(--accent)'
            : active
              ? 'inset 0 0 0 2px var(--fg)'
              : stack?.expanded
                ? 'inset 0 -3px 0 0 var(--ai)'
                : undefined,
        }}
      >
        {photo.thumb_ready && !failed ? (
          <img
            src={thumbUrl(photo, 256)}
            alt=""  // the cell's own label already starts with the file name
            loading="lazy"
            decoding="async"
            draggable={false}
            onError={() => setFailed(true)}
            className="h-full w-full object-contain transition-opacity"
            style={{ opacity: missing ? 0.35 : rejected ? 0.4 : 1, filter: missing ? 'grayscale(1)' : undefined }}
          />
        ) : failed ? (
          <div className="flex h-full w-full items-center justify-center text-faint">
            <ImageOff size={20} />
          </div>
        ) : (
          <div className="skeleton h-full w-full" />
        )}
        {selected && <div className="pointer-events-none absolute inset-0 bg-accent/10" />}
        {missing && (
          <div
            className="pointer-events-none absolute inset-0 flex items-center justify-center text-warning"
            title={t('files.missing')}
            aria-hidden="true"
            data-testid="missing-badge"
          >
            <Unlink size={size >= 100 ? 26 : 16} className="drop-shadow" />
          </div>
        )}
        <div className="pointer-events-none absolute top-1 left-1 flex flex-col items-start gap-1" aria-hidden="true">
          <div className="flex items-center gap-1">
            {photo.flag === 1 && <FlagIcon size={14} className="text-success drop-shadow" fill="currentColor" aria-hidden="true" />}
            {rejected && <Ban size={14} className="text-danger drop-shadow" aria-hidden="true" />}
            {photo.has_edits && (
              <span className="rounded bg-black/65 px-1 text-[11px] leading-4 text-accent" data-testid="edit-badge" title={t('edit.edited')}>
                ✎
              </span>
            )}
            {photo.duplicate_group_id != null && (
              <span
                className="rounded bg-black/65 px-1 text-[11px] leading-4 text-warning"
                data-testid="duplicate-badge"
                data-kind={photo.duplicate_kind}
                title={t(photo.duplicate_kind === 'exact' ? 'duplicates.exact' : 'duplicates.near')}
              >
                ⧉
              </span>
            )}
          </div>
          {badges.length > 0 && (
            <div className="flex gap-0.5 rounded bg-black/65 px-1 text-[11px] leading-4" data-testid="issue-badges">
              {badges.map((b) => (
                <span key={b.issue} className={b.tone} title={t(`issue.${b.label}`)}>
                  {b.glyph}
                </span>
              ))}
            </div>
          )}
        </div>
        {photo.color_label && (
          <span
            className="pointer-events-none absolute top-1 right-1 h-2.5 w-2.5 rounded-full ring-1 ring-black/50"
            style={{ background: colorVar(photo.color_label) }}
            title={t(`color.${photo.color_label}`)}
            aria-hidden="true"
          />
        )}
        {showBadges && (
          <>
            {photo.user_rating ? (
              <span className="tnum pointer-events-none absolute bottom-1 left-1 rounded bg-black/65 px-1 text-[11px] leading-4 text-accent" aria-hidden="true">
                {'★'.repeat(photo.user_rating)}
              </span>
            ) : photo.ai_rating !== null ? (
              <span
                className="tnum pointer-events-none absolute bottom-1 left-1 rounded bg-black/65 px-1 text-[11px] leading-4 text-ai"
                title={`${t('inspector.aiRating')}${aiScoreTip(t, photo.ai_score)}`}
                data-testid="ai-badge"
                aria-hidden="true"
              >
                ☆{photo.ai_rating.toFixed(1)}
              </span>
            ) : null}
            {paired && (
              <span
                className="pointer-events-none absolute right-1 bottom-1 rounded bg-accent px-1 text-[10px] leading-4 font-semibold text-accent-fg"
                title={t('files.pairTitle')}
                data-testid="pair-badge"
                aria-hidden="true"
              >
                {t('files.pair')}
              </span>
            )}
            {!paired && photo.format !== 'jpeg' && (
              <span
                className={`pointer-events-none absolute right-1 bottom-1 rounded px-1 text-[10px] leading-4 font-semibold uppercase ${
                  photo.format === 'raw' ? 'bg-accent text-accent-fg' : 'bg-black/65 text-white/85'
                }`}
                aria-hidden="true"
              >
                {photo.format}
              </span>
            )}
          </>
        )}
        {stack && stack.isCover && (
          <button
            type="button"
            tabIndex={-1} // keyboard users toggle stacks with S
            className="absolute top-1 left-1/2 flex -translate-x-1/2 items-center gap-1 rounded-full bg-ai px-1.5 text-[11px] leading-4 font-semibold text-white shadow"
            style={{ transform: 'translateX(-50%)' }}
            title={stack.expanded ? t('stack.collapse') : t('stack.expand', { n: stack.count })}
            aria-label={stack.expanded ? t('stack.collapse') : t('stack.expand', { n: stack.count })}
            aria-expanded={stack.expanded}
            data-testid="stack-badge"
            onClick={(e) => {
              e.stopPropagation()
              onToggleStack(stack.burstId)
            }}
            onDoubleClick={(e) => e.stopPropagation()}
          >
            <Layers size={11} />
            <span className="tnum">{stack.count}</span>
          </button>
        )}
      </div>
    </div>
  )
}, (a, b) =>
  a.photo === b.photo &&
  a.size === b.size &&
  a.selected === b.selected &&
  a.active === b.active &&
  a.onToggleStack === b.onToggleStack &&
  a.t === b.t &&
  sameStack(a.stack, b.stack),
)

function SceneHeader({ h, onToggle }: { h: HeaderItem; onToggle: (id: number) => void }) {
  const { t, i18n } = useTranslation()
  const Icon = h.collapsed ? ChevronRight : ChevronDown
  return (
    <button
      type="button"
      tabIndex={-1} // the grid is one tab stop; pointer / screen-reader activation only
      className="flex h-full w-full items-center gap-2 border-b border-line/70 px-1 text-left text-muted hover:text-fg"
      onClick={(e) => {
        e.stopPropagation()
        onToggle(h.sceneId)
      }}
      aria-expanded={!h.collapsed}
      data-testid="scene-header"
    >
      <Icon size={14} aria-hidden="true" />
      <span className="font-medium text-fg">{t('scene.title', { n: h.index })}</span>
      <span className="tnum">
        {formatClock(h.startAt, i18n.language, h.offsetMin)} – {formatClock(h.endAt, i18n.language, h.offsetMin, false)}
      </span>
      <span className="tnum text-faint">{t('scene.counts', { groups: h.groupCount, photos: h.photoCount })}</span>
    </button>
  )
}

interface Props {
  items: GridItem[]
  onOpen: (id: number) => void
  onToggleStack: (burstId: number) => void
  onToggleScene: (sceneId: number) => void
  onContext?: (e: React.MouseEvent, id: number) => void
  /** Filters / search are narrowing the list: an empty grid then offers to clear them. */
  filtered?: boolean
  /** How many photos of the session the filters hide (null = unknown). */
  hiddenCount?: number | null
}

/** Virtualized thumbnail grid with scene headers and stacks: only visible rows are mounted (20k+ items). */
export function Grid({ items, onOpen, onToggleStack, onToggleScene, onContext, filtered = false, hiddenCount = null }: Props) {
  const { t } = useTranslation()
  const [ref, size] = useElementSize<HTMLDivElement>()
  const thumbSize = useUi((s) => s.thumbSize)
  const selection = useUi((s) => s.selection)
  const activeId = useUi((s) => s.activeId)

  const innerW = Math.max(0, size.w - PAD * 2)
  const cols = Math.max(1, Math.floor((innerW + GAP) / (thumbSize + GAP)))
  const cell = Math.max(32, Math.floor((innerW - GAP * (cols - 1)) / cols))
  const rows = useMemo(() => buildRows(items, cols), [items, cols])
  const photos = useMemo(() => visiblePhotos(items), [items])
  const setGridCols = useUi((s) => s.setGridCols)
  const setGridRows = useUi((s) => s.setGridRows)
  useEffect(() => setGridCols(cols), [cols, setGridCols])
  useEffect(() => setGridRows(rows), [rows, setGridRows])

  const rowSize = useCallback((i: number) => (rows[i]?.kind === 'header' ? HEADER_H : cell + GAP), [rows, cell])
  const virt = useVirtualizer({
    count: rows.length,
    getScrollElement: () => ref.current,
    estimateSize: rowSize,
    getItemKey: (i) => {
      const r = rows[i]
      return r.kind === 'header' ? r.item.key : `p-${r.items[0].photo.id}`
    },
    overscan: 4,
    paddingStart: PAD,
    paddingEnd: PAD,
  })

  // Re-measure when the cell size or the layout changes.
  useEffect(() => {
    virt.measure()
  }, [cell, rows, virt])

  // Keep the active cell visible.
  const activeRow = useMemo(
    () => (activeId === null ? -1 : rows.findIndex((r) => r.kind === 'photos' && r.items.some((i) => i.photo.id === activeId))),
    [rows, activeId],
  )
  useEffect(() => {
    if (activeRow >= 0) virt.scrollToIndex(activeRow, { align: 'auto' })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeRow])

  // Tell the server which thumbnails are on screen (debounced) so it can prioritize them.
  const vItems = virt.getVirtualItems()
  const first = vItems[0]?.index ?? 0
  const last = vItems[vItems.length - 1]?.index ?? 0
  const rowsRef = useRef(rows)
  useEffect(() => {
    rowsRef.current = rows
  })
  useDebouncedEffect(
    () => {
      const ids: number[] = []
      for (let r = first; r <= last; r++) {
        const row = rowsRef.current[r]
        if (row?.kind !== 'photos') continue
        for (const it of row.items) if (!it.photo.thumb_ready) ids.push(it.photo.id)
      }
      if (ids.length) void api.viewport(ids.slice(0, 500)).catch(() => undefined)
    },
    [first, last, cols, rows],
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
      // a click on a cell (not on its stack button) puts keyboard focus on the grid
      if (!(e.target as HTMLElement).closest('button')) ref.current?.focus({ preventScroll: true })
      ui.setSelection(clickSelect(ui.selection, idOrder, id, { shift: e.shiftKey, ctrl: e.ctrlKey || e.metaKey }))
    },
    [idOrder, ref],
  )
  const onDoubleClick = (e: React.MouseEvent) => {
    const id = idFromEvent(e)
    if (id !== null) onOpen(id)
  }

  const empty = photos.length === 0
  return (
    <div
      ref={ref}
      className="group h-full overflow-y-auto outline-none"
      onClick={onClick}
      onDoubleClick={onDoubleClick}
      onContextMenu={(e) => {
        const id = idFromEvent(e)
        if (id !== null && onContext) onContext(e, id)
      }}
      // One tab stop: DOM focus stays on the grid, the current cell is announced via aria-activedescendant.
      role={empty ? 'region' : 'grid'}
      aria-label={t('grid.label')}
      aria-rowcount={empty ? undefined : rows.length}
      aria-multiselectable={empty ? undefined : true}
      aria-activedescendant={!empty && activeId !== null && activeRow >= 0 ? cellId(activeId) : undefined}
      tabIndex={empty ? undefined : 0}
      data-testid="grid"
    >
      {empty ? (
        filtered ? (
          <div className="flex h-full flex-col items-center justify-center gap-2 px-6 text-center text-muted" data-testid="grid-empty-filtered">
            <FilterX size={28} className="text-faint" aria-hidden="true" />
            <div className="text-fg">{t('grid.emptyFiltered')}</div>
            {hiddenCount != null && hiddenCount > 0 && <div>{t('grid.emptyHidden', { n: hiddenCount })}</div>}
            <button className="btn mt-1" onClick={() => useUi.getState().resetFilter()} data-testid="grid-empty-clear">
              {t('filter.clear')}
            </button>
          </div>
        ) : (
          <div className="flex h-full items-center justify-center text-muted">{t('grid.empty')}</div>
        )
      ) : (
        <div className="relative w-full" style={{ height: virt.getTotalSize() }}>
          {vItems.map((v) => {
            const row = rows[v.index]
            if (!row) return null
            if (row.kind === 'header') {
              return (
                <div
                  key={row.item.key}
                  role="row"
                  aria-rowindex={v.index + 1}
                  className="absolute top-0 left-0 w-full"
                  style={{ transform: `translateY(${v.start}px)`, height: HEADER_H, paddingLeft: PAD, paddingRight: PAD }}
                >
                  <div role="gridcell" aria-colspan={cols} className="h-full">
                    <SceneHeader h={row.item} onToggle={onToggleScene} />
                  </div>
                </div>
              )
            }
            return (
              <div
                key={`r${v.index}-${row.items[0]?.photo.id}`}
                role="row"
                aria-rowindex={v.index + 1}
                className="absolute top-0 left-0 flex"
                style={{ transform: `translateY(${v.start}px)`, gap: GAP, paddingLeft: PAD, height: cell }}
              >
                {row.items.map((it) => (
                  <Cell
                    key={it.photo.id}
                    photo={it.photo}
                    stack={it.stack}
                    size={cell}
                    selected={selection.ids.has(it.photo.id)}
                    active={it.photo.id === activeId}
                    onToggleStack={onToggleStack}
                    t={t}
                  />
                ))}
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}
