import { useQuery, useQueryClient } from '@tanstack/react-query'
import { ChevronLeft, ClipboardCopy, ClipboardPaste, Columns2, Download, History, Loader2, Redo2, RotateCcw, Undo2, Users, Eclipse, Check, AlertCircle } from 'lucide-react'
import { useCallback, useEffect, useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, Navigate, useNavigate, useParams } from 'react-router-dom'
import { api } from '@/api/client'
import { useMe, usePhotos } from '@/api/queries'
import { AssistantButton, AssistantHost } from '@/components/assistant/AssistantDrawer'
import { CopyDialog } from '@/components/edit/CopyDialog'
import { EditCanvas } from '@/components/edit/EditCanvas'
import { EditPanel } from '@/components/edit/EditPanel'
import { HistoryPanel } from '@/components/edit/HistoryPanel'
import { ExportDialog } from '@/components/ExportDialog'
import { Filmstrip } from '@/components/Filmstrip'
import { GenTasks } from '@/components/GenTasks'
import { HelpOverlay } from '@/components/HelpOverlay'
import { ModelConsentDialog } from '@/components/ModelConsentDialog'
import { can } from '@/lib/auth'
import { qk } from '@/lib/cache'
import { flushSaves } from '@/lib/editActions'
import { isEmptyStack } from '@/lib/edit'
import { useHistory } from '@/lib/history'
import { useEdit } from '@/stores/edit'
import { useRepair } from '@/stores/repair'
import { useToasts } from '@/stores/toasts'
import { useUi } from '@/stores/ui'
import { useEditActions } from './useEditActions'
import { useEditShortcuts } from './useEditShortcuts'

/** Edit (retouch) page: preview + panels + filmstrip. Route `/s/:sessionId/edit/:photoId`. */
export function Edit() {
  const { sessionId: rawS, photoId: rawP } = useParams()
  const sessionId = Number(rawS)
  const photoId = Number(rawP)
  const { t } = useTranslation()
  const qc = useQueryClient()
  const navigate = useNavigate()

  const role = useMe().data?.role ?? 'owner'
  const filter = useUi((s) => s.filter)
  const exportOpen = useUi((s) => s.exportOpen)
  const photosQ = usePhotos(sessionId, filter)
  const photos = useMemo(() => photosQ.data?.photos ?? [], [photosQ.data])
  const inList = photos.find((p) => p.id === photoId)
  const fallback = useQuery({
    queryKey: ['photo', photoId],
    queryFn: () => api.photo(photoId).then((r) => r.photo),
    enabled: photosQ.isSuccess && !inList,
  })
  const photo = inList ?? fallback.data

  // Load the saved stack (fresh) whenever the photo changes; pending saves go out first.
  useEffect(() => {
    let alive = true
    void flushSaves(qc)
      .then(() => qc.fetchQuery({ queryKey: qk.edits(photoId), queryFn: () => api.edits(photoId), staleTime: 0 }))
      .then((res) => {
        if (alive) useEdit.getState().load(photoId, res.stack)
      })
      .catch((err: unknown) => useToasts.getState().push('error', err instanceof Error ? err.message : String(err), 5000))
    useRepair.getState().reset()
    useUi.getState().setActive(photoId)
    return () => {
      alive = false
    }
  }, [photoId, qc])

  // Leaving the page: persist everything and clear transient edit state.
  useEffect(
    () => () => {
      void flushSaves(qc)
      useRepair.getState().reset()
      useEdit.setState({ photoId: null, loaded: false, dragging: false, compare: 'off', cropMode: false, maskOverlay: false, activeLocal: null, historyOpen: false, copyDialog: false, autoApplied: null, flash: [] })
    },
    [qc],
  )

  const back = useCallback(() => {
    void flushSaves(qc)
    navigate(`/s/${sessionId}`, { state: { keep: true } })
  }, [qc, navigate, sessionId])
  const go = useCallback((id: number) => navigate(`/s/${sessionId}/edit/${id}`, { replace: true }), [navigate, sessionId])

  const actions = useEditActions(photo, photos, back)
  useEditShortcuts(actions)

  const stack = useEdit((s) => s.stack)
  const compare = useEdit((s) => s.compare)
  const historyOpen = useEdit((s) => s.historyOpen)
  const saveState = useEdit((s) => s.saveState)
  const clipboard = useEdit((s) => s.clipboard)
  const canUndo = useHistory((s) => s.undoStack.length > 0)
  const canRedo = useHistory((s) => s.redoStack.length > 0)
  const selectionCount = useUi((s) => s.selection.ids.size)
  const edited = !isEmptyStack(stack)

  if (!can(role, 'edit')) return <Navigate to={`/s/${sessionId}`} replace />

  if (photosQ.isSuccess && !photo && fallback.isError) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3">
        <div className="text-danger">{t('library.notFound')}</div>
        <Link to={`/s/${sessionId}`} className="btn">
          {t('common.back')}
        </Link>
      </div>
    )
  }

  return (
    <div className="flex h-full flex-col" data-testid="edit-page">
      <header className="flex h-12 shrink-0 items-center gap-2 border-b border-line px-2 sm:gap-3 sm:px-3">
        <button className="btn btn-ghost" onClick={back} aria-label={t('common.back')} title={`${t('common.back')} (D / Esc)`} data-testid="edit-back">
          <ChevronLeft size={16} />
          <span className="hidden sm:inline">{t('common.back')}</span>
        </button>
        <div className="min-w-0 truncate font-semibold" data-testid="edit-title">
          {photo?.file_name ?? '…'}
        </div>
        <div className="flex items-center">
          <button className="btn btn-ghost btn-icon" disabled={!canUndo} onClick={() => void actions.undo()} aria-label={t('keys.undo')} title={`${t('keys.undo')} (Ctrl+Z)`} data-testid="edit-undo">
            <Undo2 size={15} />
          </button>
          <button className="btn btn-ghost btn-icon" disabled={!canRedo} onClick={() => void actions.redo()} aria-label={t('keys.redo')} title={`${t('keys.redo')} (Ctrl+Shift+Z)`} data-testid="edit-redo">
            <Redo2 size={15} />
          </button>
        </div>
        <div className="flex items-center gap-1" role="group" aria-label={t('edit.beforeAfter')}>
          <button className="btn" aria-pressed={compare === 'split'} onClick={actions.toggleSplit} data-testid="compare-split" title={t('edit.split')}>
            <Columns2 size={14} />
            <span className="hidden md:inline">{t('edit.split')}</span>
          </button>
          <button className="btn" aria-pressed={compare === 'original'} onClick={actions.toggleOriginal} data-testid="compare-toggle" title={`${t('edit.toggleOriginal')} (\\)`}>
            <Eclipse size={14} />
            <span className="hidden md:inline">{t('edit.toggleOriginal')}</span>
            <kbd>\</kbd>
          </button>
        </div>
        <button className="btn" aria-pressed={historyOpen} onClick={() => useEdit.getState().setHistoryOpen(!historyOpen)} data-testid="history-toggle">
          <History size={14} />
          <span className="hidden md:inline">{t('edit.history')}</span> ▸
        </button>
        <button className="btn" disabled={!edited} onClick={actions.resetAll} data-testid="reset-all" title={t('edit.resetAll')}>
          <RotateCcw size={14} />
          <span className="hidden lg:inline">{t('edit.resetAll')}</span>
        </button>
        <div className="ml-auto flex items-center gap-2">
          <span className="flex items-center gap-1 text-xs text-muted" data-testid="save-state" aria-live="polite">
            {saveState === 'saving' ? <Loader2 size={12} className="animate-spin" /> : saveState === 'error' ? <AlertCircle size={12} className="text-danger" /> : <Check size={12} className="text-success" />}
            <span className="hidden sm:inline">{t(`edit.save_${saveState}`)}</span>
          </span>
          <AssistantButton />
          <button className="btn" onClick={() => useUi.getState().setExportOpen(true)} title={`${t('top.export')} (Ctrl+E)`}>
            <Download size={14} />
            <span className="hidden sm:inline">{t('top.export')}</span>
          </button>
        </div>
      </header>

      <div className="relative flex min-h-0 flex-1">
        <main className="min-w-0 flex-1">{photo ? <EditCanvas photo={photo} /> : <div className="flex h-full items-center justify-center text-muted"><Loader2 className="animate-spin" size={18} /></div>}</main>
        <aside className="w-[320px] shrink-0 overflow-y-auto border-l border-line bg-panel" aria-label={t('edit.panel')}>
          {photo && <EditPanel photo={photo} />}
        </aside>
        {historyOpen && <HistoryPanel />}
      </div>

      <div className="flex h-9 shrink-0 items-center gap-2 border-t border-line bg-panel px-3" data-testid="edit-toolbar">
        <button className="btn" onClick={actions.copy} title={`${t('edit.copySettings')} (Ctrl+Shift+C)`} data-testid="copy-settings">
          <ClipboardCopy size={14} />
          {t('edit.copySettings')}
          <span className="hidden text-faint lg:inline">Ctrl+Shift+C</span>
        </button>
        <button className="btn" disabled={!clipboard} onClick={() => void actions.paste()} title={`${t('edit.pasteSettings')} (Ctrl+Shift+V)`} data-testid="paste-settings">
          <ClipboardPaste size={14} />
          {selectionCount > 1 ? t('edit.pasteToSelected', { n: selectionCount }) : t('edit.pasteSettings')}
          <span className="hidden text-faint lg:inline">Ctrl+Shift+V</span>
        </button>
        <button className="btn" onClick={() => void actions.sync()} data-testid="sync-group" title={t('edit.syncHint')}>
          <Users size={14} />
          {t('edit.syncGroup')}
        </button>
        <span className="ml-auto text-xs">
          <GenTasks />
        </span>
        <span className="text-xs text-muted">{photos.length > 0 && photo ? `${photos.findIndex((p) => p.id === photo.id) + 1} / ${photos.length}` : ''}</span>
      </div>
      <Filmstrip photos={photos} activeId={photoId} onPick={go} />

      <HelpOverlay />
      <ModelConsentDialog />
      <AssistantHost sessionId={sessionId} photoId={photoId} />
      <CopyDialog />
      <ExportDialog open={exportOpen} onOpenChange={useUi.getState().setExportOpen} selectedIds={[photoId]} allIds={photos.map((p) => p.id)} />
    </div>
  )
}
