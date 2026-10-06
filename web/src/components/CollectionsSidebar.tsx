import { useMutation, useQueryClient } from '@tanstack/react-query'
import { FolderHeart, HeartHandshake, Library, Pencil, Plus, Trash2, X } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'
import { api } from '@/api/client'
import { useCollectionCount, useCollections } from '@/api/queries'
import type { Collection } from '@/api/types'
import { qk } from '@/lib/cache'
import { collectionActive, queryToFilter } from '@/lib/collections'
import { isFilterActive } from '@/lib/filter'
import { useToasts } from '@/stores/toasts'
import { useUi } from '@/stores/ui'
import { Modal } from './Modal'

/** Display name: built-ins are translated, user collections show their stored name. */
function useCollectionName(): (c: Collection) => string {
  const { t } = useTranslation()
  return (c) => (c.builtin ? t(`collections.builtin_${c.id}`, { defaultValue: c.name }) : c.name)
}

function Row({ c, sessionId }: { c: Collection; sessionId: number }) {
  const { t, i18n } = useTranslation()
  const qc = useQueryClient()
  const nameOf = useCollectionName()
  const filter = useUi((s) => s.filter)
  const count = useCollectionCount(sessionId, c.query)
  const active = collectionActive(c.query, filter)
  const [renaming, setRenaming] = useState(false)
  const [draft, setDraft] = useState('')
  const [confirmDelete, setConfirmDelete] = useState(false)
  const fail = (e: unknown) => useToasts.getState().push('error', e instanceof Error ? e.message : String(e), 5000)

  const rename = useMutation({
    mutationFn: (name: string) => api.patchCollection(c.id, { name }),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.collections }),
    onError: fail,
  })
  const del = useMutation({
    mutationFn: () => api.deleteCollection(c.id),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.collections }),
    onError: fail,
  })
  const commit = () => {
    setRenaming(false)
    const v = draft.trim()
    if (v && v !== c.name) rename.mutate(v)
  }

  return (
    <li className="group/row relative" data-testid="collection-row" data-collection={c.id}>
      {renaming ? (
        <div className="flex items-center gap-1 px-2 py-1">
          <input
            autoFocus
            className="field min-w-0 flex-1 !h-6"
            value={draft}
            aria-label={t('collections.rename')}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') commit()
              if (e.key === 'Escape') setRenaming(false)
            }}
            onBlur={commit}
            data-testid="collection-rename-input"
          />
        </div>
      ) : (
        <button
          className="flex w-full items-center gap-2 rounded-control px-2 py-1.5 text-left transition-colors hover:bg-elevated"
          style={active ? { background: 'color-mix(in srgb, var(--accent) 16%, transparent)', color: 'var(--fg)' } : undefined}
          aria-current={active ? 'true' : undefined}
          onClick={() => useUi.getState().setFilter(queryToFilter(c.query))}
          data-testid="collection-apply"
        >
          <FolderHeart size={14} className={c.builtin ? 'text-ai' : 'text-accent'} />
          <span className="min-w-0 flex-1 truncate">{nameOf(c)}</span>
          <span className="tnum text-xs text-faint" data-testid="collection-count">
            {count.data === undefined ? '…' : count.data.toLocaleString(i18n.language)}
          </span>
        </button>
      )}
      {!c.builtin && !renaming && (
        <span className="absolute top-1/2 right-10 flex -translate-y-1/2 gap-0.5 opacity-0 group-focus-within/row:opacity-100 group-hover/row:opacity-100">
          <button
            className="btn btn-ghost btn-icon !h-5 !w-5 bg-panel"
            aria-label={t('collections.rename')}
            title={t('collections.rename')}
            onClick={() => {
              setDraft(c.name)
              setRenaming(true)
            }}
            data-testid="collection-rename"
          >
            <Pencil size={11} />
          </button>
          <button className="btn btn-ghost btn-icon !h-5 !w-5 bg-panel" aria-label={t('collections.delete')} title={t('collections.delete')} onClick={() => setConfirmDelete(true)} data-testid="collection-delete">
            <Trash2 size={11} />
          </button>
        </span>
      )}
      <Modal
        open={confirmDelete}
        onOpenChange={setConfirmDelete}
        title={t('collections.deleteTitle')}
        description={t('collections.deleteDesc', { name: c.name })}
        width="max-w-sm"
        footer={
          <>
            <button className="btn" onClick={() => setConfirmDelete(false)}>
              {t('common.cancel')}
            </button>
            <button
              className="btn btn-primary"
              onClick={() => {
                setConfirmDelete(false)
                del.mutate()
              }}
              data-testid="collection-delete-confirm"
            >
              <Trash2 size={13} />
              {t('collections.delete')}
            </button>
          </>
        }
      >
        <span className="sr-only">{c.name}</span>
      </Modal>
    </li>
  )
}

/** Left sidebar of the library (doc 04 3.2): smart collections (built-in + saved) and shortcuts. */
export function CollectionsSidebar({ sessionId }: { sessionId: number }) {
  const { t } = useTranslation()
  const q = useCollections()
  const filter = useUi((s) => s.filter)
  const resetFilter = useUi((s) => s.resetFilter)
  const setSidebarOpen = useUi((s) => s.setSidebarOpen)
  const list = q.data ?? []
  const anyActive = list.some((c) => collectionActive(c.query, filter))

  return (
    <nav className="flex h-full w-[208px] flex-col border-r border-line bg-panel" aria-label={t('collections.title')} data-testid="collections-sidebar">
      <div className="flex h-8 shrink-0 items-center gap-1 px-3">
        <span className="min-w-0 flex-1 truncate text-[12px] font-semibold tracking-wide text-muted uppercase">{t('collections.title')}</span>
        <button className="btn btn-ghost btn-icon !h-6 !w-6 lg:hidden" aria-label={t('common.close')} onClick={() => setSidebarOpen(false)}>
          <X size={13} />
        </button>
      </div>
      <ul className="min-h-0 flex-1 overflow-y-auto px-1 pb-2">
        <li>
          <button
            className="flex w-full items-center gap-2 rounded-control px-2 py-1.5 text-left transition-colors hover:bg-elevated"
            style={!isFilterActive(filter) ? { background: 'color-mix(in srgb, var(--accent) 16%, transparent)' } : undefined}
            onClick={resetFilter}
            data-testid="collection-all"
          >
            <Library size={14} className="text-muted" />
            <span className="flex-1 truncate">{t('collections.all')}</span>
          </button>
        </li>
        {q.isPending && <li className="px-2 py-2 text-xs text-faint">{t('library.loading')}</li>}
        {q.isError && <li className="px-2 py-2 text-xs text-danger">{(q.error as Error).message}</li>}
        {list.map((c) => (
          <Row key={c.id} c={c} sessionId={sessionId} />
        ))}
      </ul>
      <div className="flex shrink-0 flex-col gap-1 border-t border-line p-2">
        <button
          className="btn w-full justify-start"
          disabled={!isFilterActive(filter) || anyActive}
          title={!isFilterActive(filter) ? t('collections.saveNeedsFilter') : undefined}
          onClick={() => useUi.getState().setSaveCollectionOpen(true)}
          data-testid="collection-save"
        >
          <Plus size={13} />
          {t('collections.save')}
        </button>
        <Link to="/taste" className="btn btn-ghost w-full justify-start" data-testid="taste-link">
          <HeartHandshake size={13} />
          {t('taste.title')}
        </Link>
      </div>
    </nav>
  )
}
