import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Database, Loader2, RotateCcw } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { api } from '@/api/client'
import type { CatalogBackup, CatalogStatus } from '@/api/types'
import { BACKUPS_KEPT, CATALOG_KEY, catalogRefetch, formatWhen } from '@/lib/catalog'
import { errorText } from '@/lib/errors'
import { formatBytes } from '@/lib/format'
import { useToasts } from '@/stores/toasts'
import { Card, ConfirmDialog, Row } from './controls'

/** Backups with a restore button each (a confirmation first; the page reloads afterwards). Unreadable catalogs are listed but cannot be restored. */
export function BackupList({ backups, onRestored }: { backups: CatalogBackup[]; onRestored: (s: CatalogStatus) => void }) {
  const { t, i18n } = useTranslation()
  const [ask, setAsk] = useState<CatalogBackup | null>(null)
  const [busy, setBusy] = useState(false)
  const restore = async () => {
    if (!ask) return
    setBusy(true)
    try {
      const s = await api.catalogRestore(ask.name)
      useToasts.getState().push('success', t('catalog.restored'), 3500)
      setAsk(null)
      onRestored(s)
    } catch (e) {
      useToasts.getState().push('error', errorText(e), 8000)
    } finally {
      setBusy(false)
    }
  }
  return (
    <>
      <ul className="flex flex-col divide-y divide-line" data-testid="catalog-backups">
        {backups.map((b) => (
          <li key={b.name} className="flex items-center gap-3 px-4 py-2" data-testid="catalog-backup">
            <div className="min-w-0 flex-1">
              <div className="tnum font-medium">{formatWhen(b.created_at, i18n.language)}</div>
              <div className="truncate text-xs text-muted" title={b.name}>
                {t(`catalog.kind_${b.kind}`)} · {formatBytes(b.bytes)}
              </div>
            </div>
            {b.kind !== 'corrupt' && (
              <button className="btn" disabled={busy} onClick={() => setAsk(b)} data-testid="catalog-restore">
                <RotateCcw size={13} />
                {t('catalog.restore')}
              </button>
            )}
          </li>
        ))}
      </ul>
      <ConfirmDialog
        open={ask !== null}
        onOpenChange={(o) => {
          if (!o && !busy) setAsk(null)
        }}
        title={t('catalog.restoreTitle', { date: ask ? formatWhen(ask.created_at, i18n.language) : '' })}
        description={t('catalog.restoreDesc')}
        confirmLabel={busy ? t('catalog.restoring') : t('catalog.restore')}
        confirmDisabled={busy}
        danger
        testId="catalog-restore-confirm"
        onConfirm={() => void restore()}
      />
    </>
  )
}

/** 目录库备份: integrity state, "back up now" and the backups (docs/02 §8). */
export function CatalogSection() {
  const { t, i18n } = useTranslation()
  const qc = useQueryClient()
  const q = useQuery({ queryKey: CATALOG_KEY, queryFn: api.catalog, staleTime: 0, refetchInterval: (x) => catalogRefetch(x.state.data) })
  const [busy, setBusy] = useState(false)
  const st = q.data

  const backupNow = async () => {
    setBusy(true)
    try {
      await api.catalogBackup()
      await qc.invalidateQueries({ queryKey: CATALOG_KEY })
      useToasts.getState().push('success', t('settings.catalog.backedUp'), 2500)
    } catch (e) {
      useToasts.getState().push('error', errorText(e), 6000)
    } finally {
      setBusy(false)
    }
  }

  if (q.isError) return <div className="text-danger">{errorText(q.error)}</div>
  if (!st) return <Loader2 size={16} className="animate-spin text-muted" />
  const bad = st.state === 'damaged' || st.state === 'recovery'
  return (
    <div className="flex flex-col gap-4" data-testid="settings-catalog">
      <Card title={t('settings.catalog.title')} hint={t('settings.catalog.hint', { n: BACKUPS_KEPT })}>
        <Row label={t('settings.catalog.state')} hint={st.problems.length > 0 ? st.problems.join('; ') : undefined} testId="catalog-state">
          <span className={`text-xs ${bad ? 'font-semibold text-danger' : st.state === 'ok' ? 'text-success' : 'text-muted'}`}>{t(`catalog.state_${st.state}`)}</span>
        </Row>
        <Row label={t('settings.catalog.last')} hint={st.backup_dir}>
          <span className="tnum text-xs text-muted">{st.last_backup_at ? formatWhen(st.last_backup_at, i18n.language) : t('settings.catalog.never')}</span>
          <button className="btn" disabled={busy || st.state === 'recovery'} onClick={() => void backupNow()} data-testid="catalog-backup-now">
            {busy ? <Loader2 size={13} className="animate-spin" /> : <Database size={13} />}
            {t('settings.catalog.backupNow')}
          </button>
        </Row>
      </Card>
      <Card title={t('settings.catalog.backups')} hint={t('settings.catalog.backupsHint')}>
        {st.backups.length === 0 ? <div className="px-4 py-3 text-xs text-muted">{t('catalog.noBackups')}</div> : <BackupList backups={st.backups} onRestored={() => window.location.reload()} />}
      </Card>
    </div>
  )
}
