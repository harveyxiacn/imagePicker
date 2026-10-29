import { useVirtualizer } from '@tanstack/react-virtual'
import { Ban, Flag as FlagIcon, ImageOff, Layers, Unlink } from 'lucide-react'
import { Fragment, memo, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { TFunction } from 'i18next'
import { api, thumbUrl } from '@/api/client'
import type { Photo } from '@/api/types'
import { issueBadges } from '@/lib/ai'
import { colX, locate, packRows, STRIP_H } from '@/lib/gridPack'
import { useDebouncedEffect, useElementSize } from '@/lib/hooks'
import { aiScoreTip } from '@/lib/score'
import { clickSelect } from '@/lib/selection'
import { visiblePhotos, type GridItem, type StackInfo } from '@/lib/stacks'
import { useUi } from '@/stores/ui'
import { colorVar } from '@/lib/colors'
import { FoldedTile, SegmentDivider, SegmentHeader } from './GridSegments'

const GAP = 4
const PAD = 8

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
        role="gridcell"
        aria-selected={selected}
        className="relative h-full w-full cursor-pointer overflow-hidden rounded-[2px] bg-panel"
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
            alt={photo.file_name}
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
            aria-label={t('files.missing')}
            data-testid="missing-badge"
          >
            <Unlink size={size >= 100 ? 26 : 16} className="drop-shadow" />
          </div>
        )}
        <div className="pointer-events-none absolute top-1 left-1 flex flex-col items-start gap-1">
          <div className="flex items-center gap-1">
            {photo.flag === 1 && <FlagIcon size={14} className="text-success drop-shadow" fill="currentColor" />}
            {rejected && <Ban size={14} className="text-danger drop-shadow" />}
            {photo.has_edits && (
              <span className="rounded bg-black/65 px-1 text-[11px] leading-4 text-accent" data-testid="edit-badge" title={t('edit.edited')} aria-label={t('edit.edited')}>
                ✎
              </span>
            )}
            {photo.duplicate_group_id != null && (
              <span
                className="rounded bg-black/65 px-1 text-[11px] leading-4 text-warning"
                data-testid="duplicate-badge"
                data-kind={photo.duplicate_kind}
                title={t(photo.duplicate_kind === 'exact' ? 'duplicates.exact' : 'duplicates.near')}
                aria-label={t(photo.duplicate_kind === 'exact' ? 'duplicates.exact' : 'duplicates.near')}
              >
                ⧉
              </span>
            )}
          </div>
          {badges.length > 0 && (
            <div className="flex gap-0.5 rounded bg-black/65 px-1 text-[11px] leading-4" data-testid="issue-badges">
              {badges.map((b) => (
                <span key={b.issue} className={b.tone} title={t(`issue.${b.label}`)} aria-label={t(`issue.${b.label}`)}>
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
          />
        )}
        {showBadges && (
          <>
            {photo.user_rating ? (
              <span className="tnum pointer-events-none absolute bottom-1 left-1 rounded bg-black/65 px-1 text-[11px] leading-4 text-accent">
                {'★'.repeat(photo.user_rating)}
              </span>
            ) : photo.ai_rating !== null ? (
              <span
                // light tint of the AI hue on a near-opaque chip: readable over any photo, in every theme
                className="tnum pointer-events-none absolute bottom-1 left-1 rounded bg-black/80 px-1 text-[12px] leading-4 font-semibold"
                style={{ color: 'color-mix(in srgb, var(--ai) 40%, #fff)' }}
                title={`${t('inspector.aiRating')}${aiScoreTip(t, photo.ai_score)}`}
                data-testid="ai-badge"
              >
                ☆{photo.ai_rating.toFixed(1)}
              </span>
            ) : null}
            {paired && (
              <span
                className="pointer-events-none absolute right-1 bottom-1 rounded bg-accent px-1 text-[10px] leading-4 font-semibold text-accent-fg"
                title={t('files.pairTitle')}
                data-testid="pair-badge"
              >
                {t('files.pair')}
              </span>
            )}
            {!paired && photo.format !== 'jpeg' && (
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
        {stack && stack.isCover && (
          <button
            type="button"
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

interface Props {
  items: GridItem[]
  onOpen: (id: number) => void
  onToggleStack: (burstId: number) => void
  onToggleScene: (sceneId: number) => void
  onContext?: (e: React.MouseEvent, id: number) => void
}

/**
 * Virtualized thumbnail grid with scene headers and stacks: only visible rows are mounted (20k+ items).
 * Scenes are packed side by side on rows (see `packRows`), each with a compact header above its cells.
 */
export function Grid({ items, onOpen, onToggleStack, onToggleScene, onContext }: Props) {
  const { t } = useTranslation()
  const [ref, size] = useElementSize<HTMLDivElement>()
  const thumbSize = useUi((s) => s.thumbSize)
  const selection = useUi((s) => s.selection)
  const activeId = useUi((s) => s.activeId)

  const innerW = Math.max(0, size.w - PAD * 2)
  const cols = Math.max(1, Math.floor((innerW + GAP) / (thumbSize + GAP)))
  const cell = Math.max(32, Math.floor((innerW - GAP * (cols - 1)) / cols))
  const rows = useMemo(() => packRows(items, cols), [items, cols])
  const photos = useMemo(() => visiblePhotos(items), [items])
  const setGridCols = useUi((s) => s.setGridCols)
  const setGridRows = useUi((s) => s.setGridRows)
  useEffect(() => setGridCols(cols), [cols, setGridCols])
  useEffect(() => setGridRows(rows), [rows, setGridRows])

  const rowSize = useCallback((i: number) => (rows[i]?.heads.length ? STRIP_H : 0) + cell + GAP, [rows, cell])
  const virt = useVirtualizer({
    count: rows.length,
    getScrollElement: () => ref.current,
    estimateSize: rowSize,
    getItemKey: (i) => rows[i].key,
    overscan: 4,
    paddingStart: PAD,
    paddingEnd: PAD,
  })

  // Re-measure when the cell size or the layout changes.
  useEffect(() => {
    virt.measure()
  }, [cell, rows, virt])

  // Keep the active cell visible.
  const activeRow = useMemo(() => locate(rows, activeId)?.row ?? -1, [rows, activeId])
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
        for (const c of rowsRef.current[r]?.cells ?? []) if (c.kind === 'photo' && !c.item.photo.thumb_ready) ids.push(c.item.photo.id)
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
      onContextMenu={(e) => {
        const id = idFromEvent(e)
        if (id !== null && onContext) onContext(e, id)
      }}
      role="grid"
      aria-label={t('grid.label')}
      aria-rowcount={rows.length}
      data-testid="grid"
    >
      {rows.length === 0 ? (
        <div className="flex h-full items-center justify-center text-muted">{t('grid.empty')}</div>
      ) : (
        <div className="relative w-full" style={{ height: virt.getTotalSize() }}>
          {vItems.map((v) => {
            const row = rows[v.index]
            if (!row) return null
            const strip = row.heads.length > 0 ? STRIP_H : 0
            return (
              <div
                key={row.key}
                role="presentation"
                className="absolute top-0 left-0 w-full"
                style={{ transform: `translateY(${v.start}px)`, height: strip + cell, paddingLeft: PAD, paddingRight: PAD }}
              >
                {strip > 0 && (
                  <div role="row" className="relative" style={{ height: STRIP_H }}>
                    {row.heads.map((h) => (
                      <SegmentHeader key={h.header.key} head={h} cell={cell} gap={GAP} onToggle={onToggleScene} />
                    ))}
                  </div>
                )}
                <div role="row" className="relative flex" style={{ gap: GAP, height: cell }}>
                  {row.cells.map((c, ci) => (
                    <Fragment key={c.kind === 'photo' ? c.item.photo.id : `f-${c.header.key}`}>
                      {c.split && <SegmentDivider x={colX(ci, cell, GAP)} gap={GAP} height={cell} />}
                      {c.kind === 'folded' ? (
                        <FoldedTile header={c.header} size={cell} onToggle={onToggleScene} />
                      ) : (
                        <Cell
                          photo={c.item.photo}
                          stack={c.item.stack}
                          size={cell}
                          selected={selection.ids.has(c.item.photo.id)}
                          active={c.item.photo.id === activeId}
                          onToggleStack={onToggleStack}
                          t={t}
                        />
                      )}
                    </Fragment>
                  ))}
                </div>
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}
