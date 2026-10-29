import { Aperture, CalendarRange, Tag } from 'lucide-react'
import { useRef, useState, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'
import { useFilterOptions, useSession } from '@/api/queries'
import { SHARP_STATES } from '@/api/types'
import { isIsoDate, msToDate, setDateRange, toggleValue } from '@/lib/filter'
import { usePopover } from '@/lib/usePopover'
import { useUi } from '@/stores/ui'

/** Button + popover shell shared by the lens / keyword / date filters (same pattern as the device filter). */
function FilterPopover({
  testId,
  icon,
  label,
  count,
  children,
  onReset,
  canReset,
}: {
  testId: string
  icon: ReactNode
  label: string
  count: number
  children: ReactNode
  onReset: () => void
  canReset: boolean
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const wrap = useRef<HTMLDivElement>(null)
  const panel = useRef<HTMLDivElement>(null)
  const pop = usePopover({ open, onClose: () => setOpen(false), panelRef: panel, wrapRef: wrap })

  return (
    <div className="relative" ref={wrap}>
      <button
        className="btn"
        aria-pressed={count > 0 || open}
        {...pop.triggerProps}
        title={label}
        onClick={() => setOpen(!open)}
        data-testid={`${testId}-button`}
      >
        {icon}
        {label}
        {count > 0 && <span className="tnum text-accent">({count})</span>}
      </button>
      {open && (
        <div
          ref={panel}
          {...pop.panelProps}
          role="dialog"
          aria-label={label}
          className="anim-pop absolute top-full left-0 z-40 mt-1.5 w-[300px] max-w-[calc(100vw-24px)] rounded-card border border-line bg-elevated p-3 shadow-[var(--shadow)]"
          data-testid={`${testId}-popover`}
        >
          {children}
          <div className="mt-2 flex justify-end gap-2 border-t border-line pt-2">
            <button className="btn btn-ghost" disabled={!canReset} onClick={onReset}>
              {t('device.reset')}
            </button>
            <button className="btn btn-primary" onClick={() => setOpen(false)}>
              {t('person.done')}
            </button>
          </div>
        </div>
      )}
    </div>
  )
}

function CheckList({ items, selected, onToggle }: { items: Array<{ key: string; count: number }>; selected: string[]; onToggle: (k: string) => void }) {
  const { i18n } = useTranslation()
  // a selected value that is no longer in the session stays listed so it can be unticked
  const gone = selected.filter((s) => !items.some((i) => i.key === s)).map((key) => ({ key, count: 0 }))
  return (
    <ul className="flex max-h-[260px] flex-col gap-0.5 overflow-y-auto">
      {[...items, ...gone].map((i) => (
        <li key={i.key}>
          <label className="flex cursor-pointer items-center gap-2 rounded-control px-1.5 py-1 hover:bg-panel">
            <input type="checkbox" checked={selected.includes(i.key)} onChange={() => onToggle(i.key)} />
            <span className="min-w-0 flex-1 truncate" title={i.key}>
              {i.key}
            </span>
            <span className="tnum text-xs text-faint">{i.count.toLocaleString(i18n.language)}</span>
          </label>
        </li>
      ))}
    </ul>
  )
}

/** Lens filter: the lenses present in the session. Hidden when there is nothing to choose from. */
export function LensFilterButton({ sessionId }: { sessionId: number }) {
  const { t } = useTranslation()
  const session = useSession(sessionId)
  const opts = useFilterOptions(sessionId, session.data?.photo_count)
  const selected = useUi((s) => s.filter.lens)
  const setFilter = useUi((s) => s.setFilter)
  const lenses = opts.data?.lenses ?? []
  if (selected.length === 0 && lenses.length <= 1) return null
  return (
    <FilterPopover testId="lens-filter" icon={<Aperture size={14} />} label={t('filter.lens')} count={selected.length} canReset={selected.length > 0} onReset={() => setFilter({ lens: [] })}>
      <CheckList items={lenses.map((l) => ({ key: l.lens, count: l.photo_count }))} selected={selected} onToggle={(k) => setFilter({ lens: toggleValue(selected, k) })} />
    </FilterPopover>
  )
}

/** Keyword filter: the keywords present in the session, any / all. Hidden when the session has none. */
export function TagFilterButton({ sessionId }: { sessionId: number }) {
  const { t } = useTranslation()
  const session = useSession(sessionId)
  const opts = useFilterOptions(sessionId, session.data?.photo_count)
  const selected = useUi((s) => s.filter.tags)
  const mode = useUi((s) => s.filter.tagMode)
  const setFilter = useUi((s) => s.setFilter)
  const tags = opts.data?.tags ?? []
  if (selected.length === 0 && tags.length === 0) return null
  return (
    <FilterPopover testId="tag-filter" icon={<Tag size={14} />} label={t('filter.tags')} count={selected.length} canReset={selected.length > 0} onReset={() => setFilter({ tags: [] })}>
      <CheckList items={tags.map((x) => ({ key: x.tag, count: x.photo_count }))} selected={selected} onToggle={(k) => setFilter({ tags: toggleValue(selected, k) })} />
      <label className="mt-2 flex cursor-pointer items-center gap-2 border-t border-line pt-2 text-xs text-muted">
        <input type="checkbox" checked={mode === 'all'} onChange={(e) => setFilter({ tagMode: e.target.checked ? 'all' : 'any' })} data-testid="tag-mode-all" />
        {t('filter.tagsAll')}
      </label>
    </FilterPopover>
  )
}

/** Capture date range: two date inputs bounded by the session's first and last photo. Hidden without capture times. */
export function DateFilterButton({ sessionId }: { sessionId: number }) {
  const { t } = useTranslation()
  const session = useSession(sessionId)
  const opts = useFilterOptions(sessionId, session.data?.photo_count)
  const from = useUi((s) => s.filter.takenFrom)
  const to = useUi((s) => s.filter.takenTo)
  const setFilter = useUi((s) => s.setFilter)
  const min = opts.data?.taken_min != null ? msToDate(opts.data.taken_min) : undefined
  const max = opts.data?.taken_max != null ? msToDate(opts.data.taken_max) : undefined
  if (from === null && to === null && min === undefined) return null
  const set = (end: 'from' | 'to', v: string) => setFilter(setDateRange({ takenFrom: from, takenTo: to }, end, v))
  const count = (from !== null ? 1 : 0) + (to !== null ? 1 : 0)
  return (
    <FilterPopover testId="date-filter" icon={<CalendarRange size={14} />} label={t('filter.date')} count={count} canReset={count > 0} onReset={() => setFilter({ takenFrom: null, takenTo: null })}>
      <div className="flex flex-col gap-2">
        <label className="flex items-center justify-between gap-2">
          <span className="text-muted">{t('filter.dateFrom')}</span>
          <input type="date" className="field" value={from ?? ''} min={min} max={to ?? max} onChange={(e) => set('from', e.target.value)} aria-label={t('filter.dateFrom')} data-testid="date-from" />
        </label>
        <label className="flex items-center justify-between gap-2">
          <span className="text-muted">{t('filter.dateTo')}</span>
          <input type="date" className="field" value={to ?? ''} min={from ?? min} max={max} onChange={(e) => set('to', e.target.value)} aria-label={t('filter.dateTo')} data-testid="date-to" />
        </label>
        {min !== undefined && max !== undefined && isIsoDate(min) && (
          <span className="text-xs text-faint">
            {min} – {max}
          </span>
        )}
      </div>
    </FilterPopover>
  )
}

/** Sharpness chips: sharp / blurry / not analysed (click again to clear). */
export function SharpFilter() {
  const { t } = useTranslation()
  const sharp = useUi((s) => s.filter.sharp)
  const setFilter = useUi((s) => s.setFilter)
  return (
    <div className="flex items-center gap-1" role="group" aria-label={t('filter.sharp')}>
      <span className="text-muted">{t('filter.sharp')}</span>
      {SHARP_STATES.map((s) => (
        <button key={s} className="btn !h-6 !px-2" aria-pressed={sharp === s} onClick={() => setFilter({ sharp: sharp === s ? null : s })} data-testid={`sharp-${s}`}>
          {t(`filter.sharp_${s}`)}
        </button>
      ))}
    </div>
  )
}
