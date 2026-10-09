import { useQueryClient } from '@tanstack/react-query'
import { HeartHandshake, History, Loader2, Sparkles, Undo2, X } from 'lucide-react'
import { useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import { useAnalysisStatus, useMe, useTaste } from '@/api/queries'
import type { Photo, Session } from '@/api/types'
import { analysisFraction, cancelAnalysis } from '@/lib/analysis'
import { can } from '@/lib/auth'
import { useHistory } from '@/lib/history'
import { Link } from 'react-router-dom'
import { useUi } from '@/stores/ui'
import { GenTasks } from './GenTasks'

interface Props {
  sessionId: number
  session: Session | undefined
  photos: Photo[]
}

export function StatusBar({ sessionId, session, photos }: Props) {
  const { t, i18n } = useTranslation()
  const qc = useQueryClient()
  const analysis = useAnalysisStatus(sessionId).data
  const running = analysis?.state === 'running'
  const pct = analysis ? Math.round(analysisFraction(analysis) * 100) : 0
  const selected = useUi((s) => s.selection.ids.size)
  const connection = useUi((s) => s.connection)
  const taste = useTaste().data
  const me = useMe().data
  const undoCount = useHistory((s) => s.undoStack.length)
  const fmt = (n: number) => n.toLocaleString(i18n.language)
  const ready = useMemo(() => photos.reduce((n, p) => n + (p.thumb_ready ? 1 : 0), 0), [photos])
  const busy = session && session.import_state !== 'ready'

  return (
    <footer className="flex h-7 shrink-0 items-center gap-4 border-t border-line bg-panel px-3 text-xs text-muted">
      {busy ? (
        <span className="flex items-center gap-1.5 text-accent">
          <Loader2 size={12} className="animate-spin" />
          {t(`import.${session.import_state}`)} · {fmt(session.photo_count)}
        </span>
      ) : (
        <span className="tnum">{t('status.thumbs', { x: fmt(ready), y: fmt(photos.length) })}</span>
      )}
      {running && analysis && (
        <span className="flex items-center gap-2 text-ai" data-testid="analysis-progress">
          <Sparkles size={12} />
          <span>
            {t(`analysis.profile_${analysis.profile ?? 'standard'}`)} · {t(`analysis.stage_${analysis.stage ?? 'analyzing'}`)}
          </span>
          <span className="h-1.5 w-32 overflow-hidden rounded bg-line">
            <span className="block h-full bg-ai transition-[width] duration-200" style={{ width: `${pct}%` }} />
          </span>
          <span className="tnum">
            {pct}%{analysis.total > 0 && ` · ${fmt(analysis.done)} / ${fmt(analysis.total)}`}
          </span>
          <button className="btn btn-ghost !h-5 !px-1.5 text-xs" onClick={() => void cancelAnalysis(qc, sessionId)} data-testid="analysis-cancel">
            <X size={11} />
            {t('common.cancel')}
          </button>
        </span>
      )}
      <GenTasks />
      {analysis?.state === 'failed' && <span className="text-danger">{analysis.error ?? t('analysis.failed')}</span>}
      {taste?.active && (
        // key = labels so the chip pops whenever personalised scoring is retrained (`taste.updated`)
        <Link
          key={taste.labels}
          to="/taste"
          className="anim-pop flex items-center gap-1 rounded-full border border-ai/40 bg-ai/10 px-2 text-ai"
          title={t('taste.chipHint', { n: taste.labels })}
          data-testid="taste-chip"
        >
          <HeartHandshake size={11} />
          {t('taste.chip')}
        </Link>
      )}
      <span className="tnum">{t('status.selected', { n: selected })}</span>
      {session && (
        <span className="tnum hidden md:inline">
          {t('status.counts', { picked: session.picked_count, rejected: session.rejected_count, rated: session.rated_count })}
        </span>
      )}
      <span className="ml-auto flex items-center gap-3">
        {undoCount > 0 && (
          <span className="flex items-center gap-1">
            <Undo2 size={12} />
            {undoCount}
          </span>
        )}
        {can(me ? me.role : 'owner', 'settings') && (
          <Link to="/settings?section=tasks" className="flex items-center gap-1 hover:text-fg" title={t('tasks.open')} data-testid="tasks-link">
            <History size={12} />
            <span className="hidden lg:inline">{t('tasks.short')}</span>
          </Link>
        )}
        {__MOCK__ && <span className="rounded bg-accent/20 px-1.5 text-accent">MOCK</span>}
        <span className="flex items-center gap-1.5" title={t(`status.conn_${connection}`)}>
          <span
            className={`h-2 w-2 rounded-full ${connection === 'open' ? 'bg-success' : connection === 'connecting' ? 'bg-warning' : 'bg-danger'}`}
          />
          {t(`status.conn_${connection}`)}
        </span>
      </span>
    </footer>
  )
}
