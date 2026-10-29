import { Info, SearchX } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import type { SearchMeta } from '@/api/types'
import { useUi } from '@/stores/ui'

/** Strip above the results when the server flagged them as weak matches: says so instead of passing them off as hits. */
export function SearchLowConfidence({ query }: { query: string }) {
  const { t } = useTranslation()
  return (
    <div className="flex shrink-0 items-center gap-3 border-b border-line bg-warning/10 px-4 py-2 text-warning" role="status" data-testid="search-low-confidence">
      <Info size={15} className="shrink-0" />
      <span className="min-w-0 flex-1">{t('search.lowConfidence', { q: query })}</span>
    </div>
  )
}

/** Library body when a search found nothing: says why (unanalysed photos cannot match) and how to widen it. */
export function SearchEmpty({ query, meta }: { query: string; meta: SearchMeta | undefined }) {
  const { t } = useTranslation()
  const setFilter = useUi((s) => s.setFilter)
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 px-6 text-center text-muted" data-testid="search-empty">
      <SearchX size={28} className="text-faint" />
      <div className="text-fg">{t('search.empty', { q: query })}</div>
      {meta && meta.searched === 0 && meta.unindexed > 0 ? (
        <div className="max-w-md">{t('search.notAnalysed', { n: meta.unindexed })}</div>
      ) : (
        <div className="max-w-md">
          {meta?.low_confidence ? `${t('search.emptyLow')} ` : ''}
          {t('search.emptyHint')}
          {meta && meta.unindexed > 0 ? ` ${t('search.someNotAnalysed', { n: meta.unindexed })}` : ''}
        </div>
      )}
      <button className="btn mt-1" onClick={() => setFilter({ search: '' })} data-testid="search-empty-clear">
        {t('search.clear')}
      </button>
    </div>
  )
}
