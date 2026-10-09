import { useQueryClient } from '@tanstack/react-query'
import { Loader2, X } from 'lucide-react'
import { useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import { cancelGen } from '@/lib/gen'
import { runningTasks, useGen } from '@/stores/gen'

/**
 * Progress of running generative tasks (best take / inpaint / enhance) for status bars and the repair panel
 * (doc 04 section 3.5), each with a cancel button (`POST /api/tasks/{id}/cancel`). `photoId` limits it to one photo.
 */
export function GenTasks({ photoId, className = '' }: { photoId?: number; className?: string }) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const tasks = useGen((s) => s.tasks)
  const running = useMemo(() => runningTasks(tasks).filter((x) => photoId === undefined || x.photoId === photoId), [tasks, photoId])
  if (running.length === 0) return null
  return (
    <span className={`flex items-center gap-3 text-ai ${className}`} data-testid="gen-progress">
      {running.map((task) => {
        const pct = task.total > 0 ? Math.min(100, Math.round((task.done / task.total) * 100)) : 0
        return (
          <span key={task.taskId} className="flex items-center gap-1.5" data-testid="gen-task" data-kind={task.kind}>
            <Loader2 size={12} className="animate-spin" />
            <span>{task.cancelling ? t('gen.cancelling') : task.label}</span>
            <span className="h-1.5 w-24 overflow-hidden rounded bg-line">
              <span className="block h-full bg-ai transition-[width] duration-200" style={{ width: `${pct}%` }} />
            </span>
            <span className="tnum">{t('gen.percent', { n: pct })}</span>
            <button
              type="button"
              className="btn btn-ghost !h-5 !px-1.5 text-xs"
              disabled={task.cancelling}
              onClick={() => void cancelGen(qc, task.taskId)}
              title={t('gen.cancelHint')}
              data-testid="gen-cancel"
            >
              <X size={11} />
              {t('common.cancel')}
            </button>
          </span>
        )
      })}
    </span>
  )
}
