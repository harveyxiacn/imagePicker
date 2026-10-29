import { ChevronDown, ChevronRight } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { formatClock } from '@/lib/format'
import { colX, spanW, type PackedHead } from '@/lib/gridPack'
import type { HeaderItem } from '@/lib/stacks'

/** Compact scene header over its first cells; the full label is in the tooltip. */
export function SegmentHeader({
  head,
  cell,
  gap,
  onToggle,
}: {
  head: PackedHead
  cell: number
  gap: number
  onToggle: (sceneId: number) => void
}) {
  const { t, i18n } = useTranslation()
  const h = head.header
  const Icon = h.collapsed ? ChevronRight : ChevronDown
  const lang = i18n.language
  const start = formatClock(h.startAt, lang, h.offsetMin, false)
  const end = formatClock(h.endAt, lang, h.offsetMin, false)
  const full = `${t('scene.title', { n: h.index })} · ${formatClock(h.startAt, lang, h.offsetMin)} – ${end} · ${t('scene.counts', {
    groups: h.groupCount,
    photos: h.photoCount,
  })}`
  return (
    <div role="columnheader" aria-colspan={head.span} className="absolute top-0 h-full" style={{ left: colX(head.col, cell, gap), width: spanW(head.span, cell, gap) }}>
      <button
        type="button"
        tabIndex={-1} // the grid is one tab stop; pointer / screen-reader activation only
        className="hit flex h-full w-full min-w-0 items-center gap-1 border-b border-line/70 text-left text-[12px] leading-4 text-muted hover:text-fg"
        onClick={(e) => {
          e.stopPropagation()
          onToggle(h.sceneId)
        }}
        onDoubleClick={(e) => e.stopPropagation()}
        aria-expanded={!h.collapsed}
        aria-label={full}
        title={full}
        data-testid="scene-header"
      >
        <Icon size={12} className="shrink-0" aria-hidden="true" />
        <span className="min-w-0 truncate">
          <span className="font-medium text-fg">{t('scene.title', { n: h.index })}</span>
          <span className="tnum ml-1.5">{start === end ? start : `${start}–${end}`}</span>
          {h.photoCount > 1 && <span className="tnum ml-1.5 text-faint">{t('scene.photos', { n: h.photoCount })}</span>}
        </span>
      </button>
    </div>
  )
}

/** One-column stand-in for a collapsed scene; a click expands it again. */
export function FoldedTile({ header, size, onToggle }: { header: HeaderItem; size: number; onToggle: (sceneId: number) => void }) {
  const { t } = useTranslation()
  return (
    <button
      type="button"
      tabIndex={-1} // the grid is one tab stop; pointer / screen-reader activation only
      className="flex shrink-0 flex-col items-center justify-center gap-1 rounded-[2px] border border-dashed border-line bg-panel text-muted hover:text-fg"
      style={{ width: size, height: size }}
      onClick={(e) => {
        e.stopPropagation()
        onToggle(header.sceneId)
      }}
      onDoubleClick={(e) => e.stopPropagation()}
      aria-expanded={false}
      aria-label={`${t('scene.title', { n: header.index })} · ${t('scene.folded')} (${header.photoCount})`}
      data-testid="scene-folded"
    >
      <span className="tnum text-lg font-medium text-fg" aria-hidden="true">+{header.photoCount}</span>
      <span className="px-2 text-center text-[12px] leading-4">{t('scene.folded')}</span>
    </button>
  )
}

/** Thin vertical rule centred in the gap before a segment that starts mid-row. */
export function SegmentDivider({ x, gap, height }: { x: number; gap: number; height: number }) {
  return <div aria-hidden="true" className="pointer-events-none absolute top-0 w-0.5 rounded-full bg-faint/50" style={{ left: x - gap / 2 - 1, height }} />
}
