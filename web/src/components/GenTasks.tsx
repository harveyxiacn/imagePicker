import { Loader2 } from 'lucide-react'
import { useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import { runningTasks, useGen } from '@/stores/gen'

/**
 * Progress of running generative tasks (best take / inpaint / enhance) for status bars (doc 04 section 3.5).
 * The contract has no task-cancel endpoint, so there is no cancel button (the task finishes in the background).
 */
export function GenTasks() {
  const { t } = useTranslation()
  const tasks = useGen((s) => s.tasks)
  const running = useMemo(() => runningTasks(tasks), [tasks])
  if (running.length === 0) return null
  return (
    <span className="flex items-center gap-3 text-ai" data-testid="gen-progress">
      {running.map((task) => {
        const pct = task.total > 0 ? Math.min(100, Math.round((task.done / task.total) * 100)) : 0
        return (
          <span key={task.taskId} className="flex items-center gap-1.5" data-testid="gen-task" data-kind={task.kind}>
            <Loader2 size={12} className="animate-spin" />
            <span>{task.label}</span>
            <span className="h-1.5 w-24 overflow-hidden rounded bg-line">
              <span className="block h-full bg-ai transition-[width] duration-200" style={{ width: `${pct}%` }} />
            </span>
            <span className="tnum">{t('gen.percent', { n: pct })}</span>
          </span>
        )
      })}
    </span>
  )
}
