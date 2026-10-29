import { useQueryClient } from '@tanstack/react-query'
import { Download, Search, X } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useModels } from '@/api/queries'
import { MAX_QUERY_CHARS, normalizeSearch, SEARCH_DEBOUNCE_MS, TEXT_MODEL, textModelState } from '@/lib/search'
import { useAnalysisUi } from '@/stores/analysis'
import { useUi } from '@/stores/ui'

/**
 * Top-bar natural-language search (PRD F2.9). Typing applies the query to the library filter after a short pause
 * (Enter applies at once, Esc clears); the result shows as a "Search: ..." pill in the filter bar. Without the text
 * model the box is disabled and offers to install it.
 */
export function SearchBox({ sessionId }: { sessionId: number }) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const search = useUi((s) => s.filter.search)
  const setFilter = useUi((s) => s.setFilter)
  const models = useModels()
  const state = textModelState(models.data)
  const [draft, setDraft] = useState(search)
  const input = useRef<HTMLInputElement>(null)

  // the pill's "x", "Clear filters" and similar change the filter from outside: follow it (state adjusted during render)
  const [seen, setSeen] = useState(search)
  if (search !== seen) {
    setSeen(search)
    if (normalizeSearch(draft) !== search) setDraft(search)
  }

  // debounce: apply the draft once typing pauses
  useEffect(() => {
    const next = normalizeSearch(draft)
    if (next === search) return
    const id = window.setTimeout(() => setFilter({ search: next }), SEARCH_DEBOUNCE_MS)
    return () => window.clearTimeout(id)
  }, [draft, search, setFilter])

  const installModel = () =>
    useAnalysisUi.getState().setConsent({
      sessionId,
      profile: 'fast',
      models: [TEXT_MODEL],
      onReady: () => void qc.invalidateQueries({ queryKey: ['models'] }),
    })

  if (state === 'missing') {
    return (
      <div className="relative ml-1 hidden min-w-0 max-w-96 flex-1 items-center gap-1.5 lg:flex" data-testid="search-box">
        <div className="relative min-w-0 flex-1">
          <Search size={14} className="absolute top-1/2 left-2 -translate-y-1/2 text-faint" />
          <input
            className="field w-full !pl-7"
            disabled
            placeholder={t('search.placeholder')}
            aria-label={t('search.label')}
            title={t('search.needsModel')}
            data-testid="search-input"
          />
        </div>
        <button className="btn shrink-0" onClick={installModel} title={t('search.needsModel')} data-testid="search-install">
          <Download size={14} />
          {t('search.install')}
        </button>
      </div>
    )
  }

  return (
    <div className="relative ml-1 hidden min-w-0 max-w-72 flex-1 lg:block" data-testid="search-box">
      <Search size={14} className="absolute top-1/2 left-2 -translate-y-1/2 text-faint" />
      <input
        ref={input}
        className="field w-full !pr-7 !pl-7"
        value={draft}
        maxLength={MAX_QUERY_CHARS}
        placeholder={t('search.placeholder')}
        aria-label={t('search.label')}
        title={t('search.hint')}
        data-testid="search-input"
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') setFilter({ search: normalizeSearch(draft) })
          else if (e.key === 'Escape' && draft) {
            e.stopPropagation()
            setDraft('')
            setFilter({ search: '' })
          }
        }}
      />
      {draft && (
        <button
          className="absolute top-1/2 right-1 -translate-y-1/2 rounded-full p-1.5 text-faint hover:text-fg"
          aria-label={t('search.clear')}
          onClick={() => {
            setDraft('')
            setFilter({ search: '' })
            input.current?.focus()
          }}
          data-testid="search-clear"
        >
          <X size={12} />
        </button>
      )}
    </div>
  )
}
