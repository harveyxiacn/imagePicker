import { useQueryClient } from '@tanstack/react-query'
import { AlertCircle, Ban, CheckCircle2, Download, Info, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { errorText } from '@/lib/errors'
import { cancelTask } from '@/lib/tasks'
import { useAnalysisUi } from '@/stores/analysis'
import { isGenKind } from '@/stores/gen'
import { useToasts } from '@/stores/toasts'

export function Toasts() {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const toasts = useToasts((s) => s.toasts)
  const tasks = useToasts((s) => s.tasks)
  const dismiss = useToasts((s) => s.dismiss)
  const pause = useToasts((s) => s.pause)
  const resume = useToasts((s) => s.resume)
  const dismissTask = useToasts((s) => s.dismissTask)
  const consentOpen = useAnalysisUi((s) => s.consent !== null)

  return (
    <div
      className="pointer-events-none fixed right-4 bottom-10 z-(--z-toast) flex w-[min(360px,calc(100vw-32px))] flex-col gap-2 max-md:right-3 max-md:bottom-[calc(72px+env(safe-area-inset-bottom,0px))] max-md:left-3 max-md:w-auto"
    >
      {Object.values(tasks)
        .filter((task) => !(task.kind === 'model_download' && consentOpen) && !isGenKind(task.kind) && task.kind !== 'analysis')
        .map((task) => {
        const pct = task.total > 0 ? Math.round((task.done / task.total) * 100) : 0
        const label =
          task.kind === 'model_download'
            ? task.state === 'done' ? t('models.ready') : task.state === 'failed' ? t('models.failed') : t('models.downloading')
            : task.state === 'done' ? t('export.done', { n: task.total }) : task.state === 'failed' ? t('export.failed') : task.state === 'cancelled' ? t('export.cancelled', { n: task.done }) : t('export.running')
        return (
          <div
            key={task.task_id}
            role={task.state === 'failed' ? 'alert' : 'status'}
            className="anim-pop pointer-events-auto rounded-card border border-line bg-elevated p-3 shadow-[var(--shadow)]"
          >
            <div className="flex items-center gap-2">
              {task.state === 'done' ? (
                <CheckCircle2 size={16} className="text-success" />
              ) : task.state === 'failed' ? (
                <AlertCircle size={16} className="text-danger" />
              ) : task.state === 'cancelled' ? (
                <Ban size={16} className="text-muted" />
              ) : (
                <Download size={16} className="text-accent-text" />
              )}
              <span className="flex-1 font-medium">{label}</span>
              <span className="tnum text-muted">
                {task.kind === 'model_download' ? `${Math.round(task.total > 0 ? (task.done / task.total) * 100 : 0)}%` : `${task.done}/${task.total}`}
              </span>
              {task.kind === 'export' && task.state === 'running' && (
                <button className="btn btn-ghost toast-hit !px-1.5 text-xs" onClick={() => void cancelTask(qc, task.task_id)} data-testid="export-cancel">
                  {t('common.cancel')}
                </button>
              )}
              <button className="btn btn-ghost btn-icon toast-hit toast-hit-icon" aria-label={t('common.close')} onClick={() => dismissTask(task.task_id)}>
                <X size={14} />
              </button>
            </div>
            {task.state === 'failed' && task.error && <div className="mt-1 text-danger">{errorText(task.error)}</div>}
            <div
              className="mt-2 h-1.5 overflow-hidden rounded bg-line"
              role="progressbar"
              aria-label={label}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={pct}
            >
              <div
                className={`h-full transition-[width] duration-200 ${task.state === 'failed' ? 'bg-danger' : task.state === 'done' ? 'bg-success' : task.state === 'cancelled' ? 'bg-muted' : 'bg-accent'}`}
                style={{ width: `${pct}%` }}
              />
            </div>
          </div>
        )
      })}
      {toasts.map((x) => (
        <div
          key={x.id}
          role={x.kind === 'error' ? 'alert' : 'status'}
          onMouseEnter={() => pause(x.id)}
          onMouseLeave={() => resume(x.id)}
          onFocus={() => pause(x.id)}
          onBlur={() => resume(x.id)}
          className="anim-pop pointer-events-auto flex items-start gap-2 rounded-card border border-line bg-elevated p-3 shadow-[var(--shadow)]"
        >
          {x.kind === 'error' ? (
            <AlertCircle size={16} className="mt-0.5 shrink-0 text-danger" />
          ) : x.kind === 'success' ? (
            <CheckCircle2 size={16} className="mt-0.5 shrink-0 text-success" />
          ) : (
            <Info size={16} className="mt-0.5 shrink-0 text-accent-text" />
          )}
          <span className="flex min-w-0 flex-1 flex-col gap-1.5">
            <span>{x.text}</span>
            {x.actions && (
              <span className="flex flex-wrap gap-1.5">
                {x.actions.map((a) => (
                  <button
                    key={a.label}
                    className="btn toast-hit text-xs"
                    onClick={() => {
                      a.onClick()
                      dismiss(x.id)
                    }}
                  >
                    {a.label}
                  </button>
                ))}
              </span>
            )}
          </span>
          <button className="btn btn-ghost btn-icon toast-hit toast-hit-icon" aria-label={t('common.close')} onClick={() => dismiss(x.id)}>
            <X size={14} />
          </button>
        </div>
      ))}
    </div>
  )
}
