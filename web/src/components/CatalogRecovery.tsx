import { useQuery } from '@tanstack/react-query'
import { AlertTriangle } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { api } from '@/api/client'
import { useMe } from '@/api/queries'
import { can } from '@/lib/auth'
import { CATALOG_KEY, catalogRefetch, needsRecovery } from '@/lib/catalog'
import { BackupList } from './settings/CatalogSection'
import { Modal } from './Modal'

/**
 * Start-up catalog check (docs/02 §8, P0-3): when `catalog.db` could not be opened (`recovery`:
 * it was moved aside and an empty catalog is in use) or failed `quick_check` (`damaged`), offer
 * to restore a backup. "Not now" hides it until the next start.
 */
export function CatalogRecovery() {
  const { t } = useTranslation()
  const me = useMe()
  const owner = !!me.data && can(me.data.role, 'settings')
  const q = useQuery({
    queryKey: CATALOG_KEY,
    queryFn: api.catalog,
    enabled: owner,
    retry: false,
    staleTime: 0,
    refetchInterval: (x) => catalogRefetch(x.state.data),
  })
  const [dismissed, setDismissed] = useState(false)
  const st = q.data
  if (!st || dismissed || !needsRecovery(st)) return null
  const recovery = st.state === 'recovery'
  const backups = st.backups.filter((b) => b.kind !== 'corrupt')
  return (
    <Modal
      open
      onOpenChange={(o) => {
        if (!o) setDismissed(true)
      }}
      title={t(recovery ? 'catalog.recoveryTitle' : 'catalog.damagedTitle')}
      description={t(recovery ? 'catalog.recoveryDesc' : 'catalog.damagedDesc')}
      width="max-w-xl"
      footer={
        <button className="btn" onClick={() => setDismissed(true)} data-testid="catalog-recovery-dismiss">
          {t('catalog.continue')}
        </button>
      }
    >
      <div className="flex flex-col gap-3" data-testid="catalog-recovery">
        {st.moved_to && <div className="text-xs text-muted">{t('catalog.movedTo', { path: st.moved_to })}</div>}
        {st.problems.length > 0 && (
          <details className="text-xs text-muted">
            <summary className="cursor-pointer">{t('catalog.details')}</summary>
            <pre className="mt-1 whitespace-pre-wrap break-all">{st.problems.join('\n')}</pre>
          </details>
        )}
        {backups.length === 0 ? (
          <div className="flex items-start gap-2 rounded-control bg-warning/10 p-2 text-xs text-warning">
            <AlertTriangle size={14} className="mt-0.5 shrink-0" />
            {t('catalog.noBackups')}
          </div>
        ) : (
          <div className="rounded-control border border-line">
            <BackupList backups={backups} onRestored={() => window.location.reload()} />
          </div>
        )}
      </div>
    </Modal>
  )
}
