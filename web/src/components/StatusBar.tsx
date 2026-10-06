import { Loader2, Undo2 } from 'lucide-react'
import { useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import type { Photo, Session } from '@/api/types'
import { useHistory } from '@/lib/history'
import { useUi } from '@/stores/ui'

interface Props {
  session: Session | undefined
  photos: Photo[]
}

export function StatusBar({ session, photos }: Props) {
  const { t, i18n } = useTranslation()
  const selected = useUi((s) => s.selection.ids.size)
  const connection = useUi((s) => s.connection)
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
