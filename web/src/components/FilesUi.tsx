import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { AlertTriangle, FolderOpen, Loader2, RefreshCw, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { api } from '@/api/client'
import type { Photo, RejectMode, RelocateReport } from '@/api/types'
import { qk } from '@/lib/cache'
import { errorText } from '@/lib/errors'
import { useToasts } from '@/stores/toasts'
import { FolderBrowser } from './FolderBrowser'
import { Modal } from './Modal'

/** Banner above the grid when photos of the session are missing on disk: relocate the folder or rescan. */
export function MissingBanner({ sessionId, photos }: { sessionId: number; photos: Photo[] }) {
  const { t } = useTranslation()
  const push = useToasts((s) => s.push)
  const [open, setOpen] = useState(false)
  const n = photos.reduce((acc, p) => acc + (p.missing ? 1 : 0), 0)
  const rescan = useMutation({
    mutationFn: () => api.rescanSession(sessionId),
    onSuccess: () => push('info', t('files.rescanStarted'), 2500),
    onError: (e) => push('error', errorText(e), 6000),
  })
  if (n === 0) return null
  return (
    <>
      <div className="flex shrink-0 items-center gap-3 border-b border-line bg-warning/10 px-4 py-2 text-warning" role="status" data-testid="missing-banner">
        <AlertTriangle size={15} className="shrink-0" />
        <span className="min-w-0 flex-1">{t('files.missingBanner', { n })}</span>
        <button className="btn" onClick={() => setOpen(true)} data-testid="relocate-open">
          <FolderOpen size={14} />
          {t('files.relocate')}
        </button>
        <button className="btn" onClick={() => rescan.mutate()} disabled={rescan.isPending}>
          {rescan.isPending ? <Loader2 size={14} className="animate-spin" /> : <RefreshCw size={14} />}
          {t('files.rescan')}
        </button>
      </div>
      <RelocateDialog sessionId={sessionId} open={open} onOpenChange={setOpen} />
    </>
  )
}

/** Point the session at the folder its photos moved to; shows what would be re-linked first. */
export function RelocateDialog({ sessionId, open, onOpenChange }: { sessionId: number; open: boolean; onOpenChange: (b: boolean) => void }) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const push = useToasts((s) => s.push)
  const [path, setPath] = useState('')
  const [browse, setBrowse] = useState(false)
  const [report, setReport] = useState<RelocateReport | null>(null)
  const [error, setError] = useState<string | null>(null)

  const preview = useMutation({
    mutationFn: (p: string) => api.relocateSession(sessionId, p, true),
    onSuccess: ({ relocate }) => {
      setReport(relocate)
      setError(null)
    },
    onError: (e) => {
      setReport(null)
      setError(errorText(e))
    },
  })
  const apply = useMutation({
    mutationFn: () => api.relocateSession(sessionId, path, false),
    onSuccess: ({ relocate }) => {
      push('success', t('files.relocated', { n: relocate.matched + relocate.changed }), 4000)
      void qc.invalidateQueries({ queryKey: qk.sessions })
      void qc.invalidateQueries({ queryKey: qk.photosAll })
      onOpenChange(false)
    },
    onError: (e) => push('error', errorText(e), 9000),
  })
  const choose = (p: string) => {
    setPath(p)
    setReport(null)
    preview.mutate(p)
  }
  return (
    <>
      <Modal
        open={open}
        onOpenChange={onOpenChange}
        title={t('files.relocateTitle')}
        description={t('files.relocateDesc')}
        footer={
          <>
            <button className="btn" onClick={() => onOpenChange(false)}>
              {t('common.cancel')}
            </button>
            <button
              className="btn btn-primary"
              disabled={!report || report.matched === 0 || apply.isPending}
              onClick={() => apply.mutate()}
              data-testid="relocate-apply"
            >
              {apply.isPending && <Loader2 size={14} className="animate-spin" />}
              {t('files.relocateApply')}
            </button>
          </>
        }
      >
        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1">
            <label htmlFor="reloc-path" className="text-muted">
              {t('files.relocateNew')}
            </label>
            <div className="flex gap-2">
              <input id="reloc-path" className="field flex-1 font-mono text-xs" value={path} readOnly placeholder={t('files.relocateChoose')} />
              <button className="btn" onClick={() => setBrowse(true)}>
                <FolderOpen size={14} />
                {t('files.relocateChoose')}
              </button>
            </div>
          </div>
          {preview.isPending && <Loader2 size={16} className="animate-spin text-muted" />}
          {report && (
            <div className="text-xs text-muted" data-testid="relocate-preview">
              {t('files.relocatePreview', { matched: report.matched, total: report.total, changed: report.changed, absent: report.absent })}
            </div>
          )}
          {error && <div className="text-xs text-danger" data-testid="relocate-error">{error}</div>}
        </div>
      </Modal>
      <FolderBrowser open={browse} onOpenChange={setBrowse} title={t('files.relocateChoose')} onSelect={choose} />
    </>
  )
}

/** Moves the session's rejected photos (and their RAW companions) to the recycle bin or a `_rejected` folder, after a confirmation. Never deletes. */
export function RejectFilesDialog({ sessionId, open, onOpenChange }: { sessionId: number; open: boolean; onOpenChange: (b: boolean) => void }) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const push = useToasts((s) => s.push)
  const [mode, setMode] = useState<RejectMode>('trash')
  const rejected = useQuery({
    queryKey: ['rejected-count', sessionId],
    queryFn: () => api.photos({ session_id: sessionId, flag: 'rejected', limit: 1 }),
    enabled: open,
  })
  const n = rejected.data?.total ?? 0
  const run = useMutation({
    mutationFn: () => api.rejectFiles({ session_id: sessionId, mode, confirm: true }),
    onSuccess: ({ result }) => {
      if (result.errors.length > 0) push('error', t('files.rejectErrors', { n: result.errors.length, first: result.errors[0] }), 9000)
      else push('success', t('files.rejectDone', { n: result.photos }), 4000)
      void qc.invalidateQueries({ queryKey: qk.sessions })
      void qc.invalidateQueries({ queryKey: qk.photosAll })
      onOpenChange(false)
    },
    onError: (e) => push('error', errorText(e), 9000),
  })
  return (
    <Modal
      open={open}
      onOpenChange={onOpenChange}
      title={t('files.rejectTitle')}
      description={n > 0 ? t('files.rejectDesc', { n }) : t('files.rejectNone')}
      footer={
        <>
          <button className="btn" onClick={() => onOpenChange(false)}>
            {t('common.cancel')}
          </button>
          <button className="btn btn-primary" disabled={n === 0 || run.isPending} onClick={() => run.mutate()} data-testid="reject-confirm">
            {run.isPending ? <Loader2 size={14} className="animate-spin" /> : <Trash2 size={14} />}
            {t('files.rejectConfirm', { n })}
          </button>
        </>
      }
    >
      <fieldset className="flex flex-col gap-3">
        {(['trash', 'folder'] as const).map((m) => (
          <label key={m} className="flex items-start gap-2">
            <input type="radio" name="reject-mode" className="mt-1" checked={mode === m} onChange={() => setMode(m)} data-testid={`reject-mode-${m}`} />
            <span>
              <span className="block font-medium">{t(m === 'trash' ? 'files.rejectTrash' : 'files.rejectFolder')}</span>
              <span className="block text-xs text-muted">{t(m === 'trash' ? 'files.rejectTrashHint' : 'files.rejectFolderHint')}</span>
            </span>
          </label>
        ))}
      </fieldset>
    </Modal>
  )
}
