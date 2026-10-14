import { SearchX } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import type { SearchMeta } from '@/api/types'
import { useUi } from '@/stores/ui'

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
