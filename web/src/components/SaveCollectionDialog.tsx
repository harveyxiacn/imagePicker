import { useQueryClient } from '@tanstack/react-query'
import { Loader2 } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { api } from '@/api/client'
import { filterToQuery } from '@/lib/collections'
import { qk } from '@/lib/cache'
import { useToasts } from '@/stores/toasts'
import { useUi } from '@/stores/ui'
import { Modal } from './Modal'

/** "Save as smart collection": stores the current library filter under a name (docs/api-contract-m4.md C.2). */
export function SaveCollectionDialog() {
  const { t } = useTranslation()
  const open = useUi((s) => s.saveCollectionOpen)
  const setOpen = useUi((s) => s.setSaveCollectionOpen)
  const filter = useUi((s) => s.filter)
  const qc = useQueryClient()
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)

  const save = async () => {
    const n = name.trim()
    if (!n || busy) return
    setBusy(true)
    try {
      await api.createCollection(n, filterToQuery(useUi.getState().filter))
      void qc.invalidateQueries({ queryKey: qk.collections })
      useToasts.getState().push('success', t('collections.saved', { name: n }), 2500)
      setName('')
      setOpen(false)
    } catch (err) {
      useToasts.getState().push('error', err instanceof Error ? err.message : String(err), 5000)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Modal
      open={open}
      onOpenChange={setOpen}
      title={t('collections.saveTitle')}
      description={t('collections.saveDesc')}
      width="max-w-sm"
      footer={
        <>
          <button className="btn" onClick={() => setOpen(false)}>
            {t('common.cancel')}
          </button>
          <button className="btn btn-primary" disabled={!name.trim() || busy} onClick={() => void save()} data-testid="collection-save-confirm">
            {busy && <Loader2 size={14} className="animate-spin" />}
            {t('common.save')}
          </button>
        </>
      }
    >
      <div className="flex flex-col gap-2">
        <input
          className="field w-full"
          autoFocus
          value={name}
          placeholder={t('collections.namePlaceholder')}
          aria-label={t('collections.name')}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && void save()}
          data-testid="collection-name-input"
        />
        <code className="rounded bg-bg px-2 py-1 text-[11px] break-all text-faint" data-testid="collection-query-preview">
          {filterToQuery(filter) || t('collections.allPhotos')}
        </code>
      </div>
    </Modal>
  )
}
