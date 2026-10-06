import { AlertCircle, CheckCircle2, Download, Info, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useAnalysisUi } from '@/stores/analysis'
import { isGenKind } from '@/stores/gen'
import { useToasts } from '@/stores/toasts'

export function Toasts() {
  const { t } = useTranslation()
  const toasts = useToasts((s) => s.toasts)
  const tasks = useToasts((s) => s.tasks)
  const dismiss = useToasts((s) => s.dismiss)
  const dismissTask = useToasts((s) => s.dismissTask)
  const consentOpen = useAnalysisUi((s) => s.consent !== null)

  return (
    <div
      className="pointer-events-none fixed right-4 bottom-10 z-[60] flex w-[min(360px,calc(100vw-32px))] flex-col gap-2"
      role="status"
      aria-live="polite"
    >
      {Object.values(tasks)
        .filter((task) => !(task.kind === 'model_download' && consentOpen) && !isGenKind(task.kind) && task.kind !== 'analysis')
        .map((task) => {
        const pct = task.total > 0 ? Math.round((task.done / task.total) * 100) : 0
        return (
          <div key={task.task_id} className="anim-pop pointer-events-auto rounded-card border border-line bg-elevated p-3 shadow-[var(--shadow)]">
            <div className="flex items-center gap-2">
              {task.state === 'done' ? (
                <CheckCircle2 size={16} className="text-success" />
              ) : task.state === 'failed' ? (
                <AlertCircle size={16} className="text-danger" />
              ) : (
                <Download size={16} className="text-accent" />
              )}
              <span className="flex-1 font-medium">
                {task.kind === 'model_download'
                  ? task.state === 'done'
                    ? t('models.ready')
                    : task.state === 'failed'
                      ? t('models.failed')
                      : t('models.downloading')
                  : task.state === 'done'
                    ? t('export.done', { n: task.total })
                    : task.state === 'failed'
                      ? t('export.failed')
                      : t('export.running')}
              </span>
              <span className="tnum text-muted">
                {task.kind === 'model_download' ? `${Math.round(task.total > 0 ? (task.done / task.total) * 100 : 0)}%` : `${task.done}/${task.total}`}
              </span>
              <button className="btn btn-ghost btn-icon !h-6 !w-6" aria-label={t('common.close')} onClick={() => dismissTask(task.task_id)}>
                <X size={14} />
              </button>
            </div>
            {task.state === 'failed' && task.error && <div className="mt-1 text-danger">{task.error}</div>}
            <div className="mt-2 h-1.5 overflow-hidden rounded bg-line">
              <div
                className={`h-full transition-[width] duration-200 ${task.state === 'failed' ? 'bg-danger' : task.state === 'done' ? 'bg-success' : 'bg-accent'}`}
                style={{ width: `${pct}%` }}
              />
            </div>
          </div>
        )
      })}
      {toasts.map((x) => (
        <div
          key={x.id}
          className="anim-pop pointer-events-auto flex items-start gap-2 rounded-card border border-line bg-elevated p-3 shadow-[var(--shadow)]"
        >
          {x.kind === 'error' ? (
            <AlertCircle size={16} className="mt-0.5 shrink-0 text-danger" />
          ) : x.kind === 'success' ? (
            <CheckCircle2 size={16} className="mt-0.5 shrink-0 text-success" />
          ) : (
            <Info size={16} className="mt-0.5 shrink-0 text-accent" />
          )}
          <span className="flex min-w-0 flex-1 flex-col gap-1.5">
            <span>{x.text}</span>
            {x.actions && (
              <span className="flex flex-wrap gap-1.5">
                {x.actions.map((a) => (
                  <button
                    key={a.label}
                    className="btn !h-6 text-xs"
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
          <button className="btn btn-ghost btn-icon !h-6 !w-6" aria-label={t('common.close')} onClick={() => dismiss(x.id)}>
            <X size={14} />
          </button>
        </div>
      ))}
    </div>
  )
}
