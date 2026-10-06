import { Star, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import type { FlagFilter, SortKey } from '@/api/types'
import { isFilterActive } from '@/lib/filter'
import { useUi } from '@/stores/ui'
import { COLOR_NAMES, colorVar } from '@/lib/colors'

const FLAGS: ('all' | FlagFilter)[] = ['all', 'picked', 'not_rejected', 'unflagged', 'rejected']
const SORTS: SortKey[] = ['taken_at', '-taken_at', 'name', 'rating']

interface Props {
  shown: number
  total: number
  showSize: boolean
}

export function FilterBar({ shown, total, showSize }: Props) {
  const { t, i18n } = useTranslation()
  const filter = useUi((s) => s.filter)
  const setFilter = useUi((s) => s.setFilter)
  const resetFilter = useUi((s) => s.resetFilter)
  const thumbSize = useUi((s) => s.thumbSize)
  const setThumbSize = useUi((s) => s.setThumbSize)
  const fmt = (n: number) => n.toLocaleString(i18n.language)
  const active = isFilterActive(filter)

  return (
    <div className="flex shrink-0 flex-wrap items-center gap-x-4 gap-y-1.5 border-b border-line px-3 py-1.5">
      <div className="flex items-center gap-1" role="group" aria-label={t('filter.rating')}>
        <span className="text-muted">{t('filter.rating')}</span>
        <Star size={13} className="text-accent" fill="currentColor" />
        <span className="text-muted">≥</span>
        {[0, 1, 2, 3, 4, 5].map((n) => (
          <button
            key={n}
            className="btn !h-6 !w-6 justify-center !px-0 tnum"
            aria-pressed={filter.ratingGte === n}
            onClick={() => setFilter({ ratingGte: n })}
            title={n === 0 ? t('filter.any') : `≥ ${n}`}
          >
            {n === 0 ? '–' : n}
          </button>
        ))}
      </div>

      <label className="flex items-center gap-1.5">
        <span className="text-muted">{t('filter.flag')}</span>
        <select
          className="field !px-1.5"
          value={filter.flag}
          onChange={(e) => setFilter({ flag: e.target.value as 'all' | FlagFilter })}
          aria-label={t('filter.flag')}
        >
          {FLAGS.map((f) => (
            <option key={f} value={f}>
              {t(`filter.flag_${f}`)}
            </option>
          ))}
        </select>
      </label>

      <div className="flex items-center gap-1.5" role="group" aria-label={t('filter.color')}>
        <span className="text-muted">{t('filter.color')}</span>
        {COLOR_NAMES.map((c) => (
          <button
            key={c}
            className="h-4 w-4 rounded-full border-2 transition-transform hover:scale-110"
            style={{ background: colorVar(c), borderColor: filter.color === c ? 'var(--fg)' : 'transparent' }}
            aria-label={t(`color.${c}`)}
            aria-pressed={filter.color === c}
            onClick={() => setFilter({ color: filter.color === c ? null : c })}
          />
        ))}
      </div>

      <label className="flex items-center gap-1.5">
        <span className="text-muted">{t('filter.sort')}</span>
        <select
          className="field !px-1.5"
          value={filter.sort}
          onChange={(e) => setFilter({ sort: e.target.value as SortKey })}
          aria-label={t('filter.sort')}
        >
          {SORTS.map((s) => (
            <option key={s} value={s}>
              {t(`filter.sort_${s}`)}
            </option>
          ))}
        </select>
      </label>

      {active && (
        <button className="btn btn-ghost" onClick={resetFilter}>
          <X size={13} />
          {t('filter.clear')}
        </button>
      )}

      <div className="ml-auto flex items-center gap-4">
        {showSize && (
          <label className="flex items-center gap-2">
            <span className="text-muted">{t('filter.size')}</span>
            <input
              type="range"
              min={96}
              max={320}
              step={8}
              value={thumbSize}
              onChange={(e) => setThumbSize(Number(e.target.value))}
              aria-label={t('filter.size')}
              className="w-28"
            />
          </label>
        )}
        <span className="tnum whitespace-nowrap text-muted" data-testid="showing">
          {t('filter.showing', { x: fmt(shown), y: fmt(total) })}
        </span>
      </div>
    </div>
  )
}
