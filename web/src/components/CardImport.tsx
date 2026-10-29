import { useMutation, useQueryClient } from '@tanstack/react-query'
import { AlertCircle, Ban, CheckCircle2, FolderOpen, HardDrive, Info, Loader2 } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { api } from '@/api/client'
import { useImportSources, useTasks } from '@/api/queries'
import type { ImportSource } from '@/api/types'
import { qk } from '@/lib/cache'
import { cardCaption, cardSummary, loadCardDest, PATTERN_PRESETS, previewPath, saveCardDest } from '@/lib/cardImport'
import { errorText } from '@/lib/errors'
import { cancelTask, isLive } from '@/lib/tasks'
import { useCardImport } from '@/stores/cardImport'
import { useToasts } from '@/stores/toasts'
import { FolderBrowser } from './FolderBrowser'
import { Modal } from './Modal'

/** Home: the memory cards that are plugged in (a DCIM folder on a removable drive), each with "Import from card". */
export function CardImportSection({ excludes }: { excludes: string[] }) {
  const { t } = useTranslation()
  const sources = useImportSources()
  const [active, setActive] = useState<ImportSource | null>(null)
  const list = sources.data?.sources ?? []
  if (list.length === 0 && !active) return null
  return (
    <section className="rounded-card border border-line bg-panel p-4" aria-label={t('card.title')} data-testid="card-section">
      <h2 className="text-base font-semibold">{t('card.title')}</h2>
      <p className="mt-0.5 mb-3 text-xs text-muted">{t('card.hint')}</p>
      <ul className="flex flex-col gap-2">
        {list.map((s) => (
          <li key={s.path} className="flex flex-wrap items-center gap-3 rounded-control border border-line bg-bg px-3 py-2" data-testid="card-source">
            <HardDrive size={18} className="shrink-0 text-accent-text" />
            <div className="min-w-0 flex-1">
              <div className="truncate font-medium">{s.label}</div>
              <div className="tnum truncate text-xs text-muted">
                {[s.path, ...cardCaption(s).map((c) => t(c.key, c.opts))].join(' · ')}
              </div>
            </div>
            <button className="btn btn-primary !h-8 px-3" onClick={() => setActive(s)} data-testid="card-import">
              {t('card.import')}
            </button>
          </li>
        ))}
      </ul>
      {active && <CardImportDialog source={active} excludes={excludes} onClose={() => setActive(null)} />}
    </section>
  )
}

function CardImportDialog({ source, excludes, onClose }: { source: ImportSource; excludes: string[]; onClose: () => void }) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const qc = useQueryClient()
  const push = useToasts((s) => s.push)
  const [dest, setDest] = useState(loadCardDest)
  const [pattern, setPattern] = useState<string>(PATTERN_PRESETS[0].pattern)
  const [title, setTitle] = useState('')
  const [browse, setBrowse] = useState(false)
  const [taskId, setTaskId] = useState<string | null>(null)
  const [stopping, setStopping] = useState(false)

  const start = useMutation({
    mutationFn: () =>
      api.importCard({
        source: source.path,
        dest,
        pattern,
        ...(title.trim() ? { title: title.trim() } : {}),
        ...(excludes.length > 0 ? { exclude: excludes } : {}),
      }),
    onSuccess: ({ task_id }) => {
      saveCardDest(dest)
      setTaskId(task_id)
      void qc.invalidateQueries({ queryKey: qk.tasks })
    },
    onError: (e) => push('error', errorText(e), 9000),
  })

  const tasks = useTasks(20, taskId !== null)
  const live = useCardImport((s) => (taskId ? s.events[taskId] : undefined))
  const rec = taskId ? tasks.data?.find((r) => r.id === taskId) : undefined
  const ended = rec !== undefined && !isLive(rec.status)
  const running = taskId !== null && !ended
  const done = live?.done ?? rec?.done ?? 0
  const total = live?.total ?? rec?.total ?? 0

  const close = () => {
    if (taskId) useCardImport.getState().clear(taskId)
    void qc.invalidateQueries({ queryKey: qk.sessions })
    onClose()
  }

  const sample = new Date()
  const summary = ended ? cardSummary(rec) : null
  const Icon = summary?.tone === 'success' ? CheckCircle2 : summary?.tone === 'error' ? AlertCircle : summary?.tone === 'warning' ? AlertCircle : summary?.tone === 'muted' && rec?.status === 'cancelled' ? Ban : Info
  const tone = summary?.tone === 'success' ? 'text-success' : summary?.tone === 'error' ? 'text-danger' : summary?.tone === 'warning' ? 'text-accent-text' : 'text-muted'

  return (
    <Modal
      open
      onOpenChange={(o) => {
        // a running copy keeps going if the dialog is closed; it can be cancelled from the task history
        if (!o) close()
      }}
      title={t('card.dialogTitle', { label: source.label })}
      width="max-w-xl"
      footer={
        running ? (
          <button
            className="btn"
            disabled={stopping || rec?.status === 'cancelling'}
            onClick={() => {
              setStopping(true)
              if (taskId) void cancelTask(qc, taskId)
            }}
            data-testid="card-cancel"
          >
            {stopping || rec?.status === 'cancelling' ? t('card.stopping') : t('common.cancel')}
          </button>
        ) : summary ? (
          <>
            <button className="btn" onClick={close}>
              {t('common.close')}
            </button>
            {summary.sessionId !== null && (
              <button
                className="btn btn-primary"
                onClick={() => {
                  const id = summary.sessionId
                  close()
                  navigate(`/s/${id}`)
                }}
                data-testid="card-open-session"
              >
                {t('card.openSession')}
              </button>
            )}
          </>
        ) : (
          <>
            <button className="btn" onClick={close}>
              {t('common.cancel')}
            </button>
            <button className="btn btn-primary" disabled={!dest || start.isPending} onClick={() => start.mutate()} data-testid="card-start">
              {start.isPending && <Loader2 size={15} className="animate-spin" />}
              {t('card.start')}
            </button>
          </>
        )
      }
    >
      {taskId === null ? (
        <div className="flex flex-col gap-4">
          <div className="text-xs text-muted">
            {source.path} · {t('card.safeHint')}
          </div>
          <div className="flex flex-col gap-1">
            <span className="text-muted">{t('card.dest')}</span>
            <div className="flex items-center gap-2">
              <code className="field min-w-0 flex-1 truncate font-mono text-xs" data-testid="card-dest">
                {dest || t('card.destNone')}
              </code>
              <button className="btn" onClick={() => setBrowse(true)} data-testid="card-dest-choose">
                <FolderOpen size={15} />
                {t('card.choose')}
              </button>
            </div>
          </div>
          <div className="flex flex-col gap-1">
            <label htmlFor="card-pattern" className="text-muted">
              {t('card.pattern')}
            </label>
            <select id="card-pattern" className="field" value={pattern} onChange={(e) => setPattern(e.target.value)} data-testid="card-pattern">
              {PATTERN_PRESETS.map((p) => (
                <option key={p.id} value={p.pattern}>
                  {t(`card.preset_${p.id}`)}
                </option>
              ))}
            </select>
            {dest && <div className="tnum truncate font-mono text-xs text-faint">{t('card.example', { path: previewPath(dest, pattern, sample) })}</div>}
          </div>
          <div className="flex flex-col gap-1">
            <label htmlFor="card-title" className="text-muted">
              {t('card.sessionName')}
            </label>
            <input id="card-title" className="field" value={title} onChange={(e) => setTitle(e.target.value)} placeholder={source.label} />
          </div>
          {excludes.length > 0 && <div className="text-xs text-faint">{t('card.excludes', { n: excludes.length })}</div>}
        </div>
      ) : running ? (
        <div className="flex flex-col gap-3" data-testid="card-progress">
          <div className="flex items-center gap-2">
            <Loader2 size={16} className="animate-spin text-accent-text" />
            <span className="flex-1">{total > 0 ? t('card.copying', { done, total }) : t('card.scanning')}</span>
          </div>
          <div className="h-1.5 overflow-hidden rounded bg-line">
            <div className="h-full bg-accent transition-[width] duration-200" style={{ width: `${total > 0 ? Math.round((done / total) * 100) : 0}%` }} />
          </div>
          <div className="text-xs text-faint">{t('card.runningHint')}</div>
        </div>
      ) : summary ? (
        <div className="flex flex-col gap-3" data-testid="card-result">
          <div className={`flex items-center gap-2 font-medium ${tone}`}>
            <Icon size={18} />
            {t(summary.headline.key, summary.headline.opts)}
          </div>
          <ul className="flex flex-col gap-1 text-muted">
            {summary.lines.map((l) => (
              <li key={l.key}>{t(l.key, l.opts)}</li>
            ))}
          </ul>
          {summary.failed.length > 0 && (
            <details>
              <summary className="cursor-pointer text-muted">{t('card.failedList')}</summary>
              <ul className="mt-1 max-h-40 overflow-auto text-xs">
                {summary.failed.map((f) => (
                  <li key={f.path} className="truncate" title={`${f.path}: ${f.error}`}>
                    <span className="font-mono">{f.path}</span> <span className="text-danger">{f.error}</span>
                  </li>
                ))}
              </ul>
            </details>
          )}
        </div>
      ) : null}
      <FolderBrowser
        open={browse}
        onOpenChange={setBrowse}
        title={t('card.chooseDest')}
        confirmLabel={t('card.useFolder')}
        onSelect={(p) => setDest(p)}
      />
    </Modal>
  )
}
