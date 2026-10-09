import { useQueryClient } from '@tanstack/react-query'
import { AlertCircle, Ban, CheckCircle2, Loader2, PowerOff, RefreshCw, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useTasks } from '@/api/queries'
import type { TaskRecord } from '@/api/types'
import { cancelTask, isLive, taskDetail, taskPercent } from '@/lib/tasks'
import { Card } from './controls'

function StatusIcon({ status }: { status: TaskRecord['status'] }) {
  switch (status) {
    case 'running':
    case 'cancelling':
      return <Loader2 size={15} className="shrink-0 animate-spin text-ai" />
    case 'done':
      return <CheckCircle2 size={15} className="shrink-0 text-success" />
    case 'failed':
      return <AlertCircle size={15} className="shrink-0 text-danger" />
    case 'cancelled':
      return <Ban size={15} className="shrink-0 text-muted" />
    default:
      return <PowerOff size={15} className="shrink-0 text-warning" />
  }
}

function TaskRow({ task }: { task: TaskRecord }) {
  const { t, i18n } = useTranslation()
  const qc = useQueryClient()
  const detail = taskDetail(task)
  const rawOp = detail?.opts.op
  const op = typeof rawOp === 'string' ? t(`tasks.op_${rawOp}`, { defaultValue: rawOp }) : undefined
  const live = isLive(task.status)
  const pct = taskPercent(task)
  const when = new Date(task.created_at).toLocaleString(i18n.language, { dateStyle: 'short', timeStyle: 'short' })
  return (
    <li className="flex flex-col gap-1 px-4 py-2.5" data-testid="task-row" data-kind={task.kind} data-status={task.status}>
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <StatusIcon status={task.status} />
        <span className="font-medium">{t(`tasks.kind_${task.kind}`, { defaultValue: task.kind })}</span>
        {detail && <span className="min-w-0 truncate text-muted">{t(detail.key, { ...detail.opts, op })}</span>}
        <span className="ml-auto flex items-center gap-2 text-xs text-muted">
          <span className="tnum">{when}</span>
          <span className={task.status === 'failed' ? 'text-danger' : task.status === 'interrupted' ? 'text-warning' : ''}>{t(`tasks.status_${task.status}`)}</span>
          {task.cancellable && (
            <button className="btn btn-ghost !h-6 !px-1.5 text-xs" onClick={() => void cancelTask(qc, task.id)} data-testid="task-cancel">
              <X size={11} />
              {t('common.cancel')}
            </button>
          )}
        </span>
      </div>
      {(live || task.total > 0) && (
        <div className="flex items-center gap-2 text-xs text-muted">
          <span className="h-1.5 flex-1 overflow-hidden rounded bg-line">
            <span
              className={`block h-full transition-[width] duration-200 ${task.status === 'failed' ? 'bg-danger' : task.status === 'done' ? 'bg-success' : live ? 'bg-ai' : 'bg-muted'}`}
              style={{ width: `${pct}%` }}
            />
          </span>
          <span className="tnum">{t('tasks.progress', { done: task.done, total: task.total })}</span>
        </div>
      )}
      {task.error && task.status !== 'cancelled' && <div className="text-xs break-words text-danger">{task.error}</div>}
    </li>
  )
}

/** 任务记录: recent generation / export / analysis tasks with their outcome; running ones can be cancelled. */
export function TasksSection() {
  const { t } = useTranslation()
  const tasks = useTasks(50)
  return (
    <div className="flex flex-col gap-4" data-testid="settings-tasks">
      <p className="text-xs text-muted">{t('tasks.hint')}</p>
      <Card>
        <div className="flex items-center justify-between px-4 py-2">
          <span className="text-xs text-muted">{t('tasks.count', { n: tasks.data?.length ?? 0 })}</span>
          <button className="btn btn-ghost !h-7 text-xs" onClick={() => void tasks.refetch()} disabled={tasks.isFetching} data-testid="tasks-refresh">
            <RefreshCw size={12} className={tasks.isFetching ? 'animate-spin' : ''} />
            {t('tasks.refresh')}
          </button>
        </div>
        {tasks.isError ? (
          <div className="px-4 py-3 text-danger">{(tasks.error as Error).message}</div>
        ) : tasks.isLoading ? (
          <div className="flex justify-center px-4 py-6 text-muted">
            <Loader2 size={16} className="animate-spin" />
          </div>
        ) : (tasks.data ?? []).length === 0 ? (
          <div className="px-4 py-6 text-center text-muted" data-testid="tasks-empty">
            {t('tasks.empty')}
          </div>
        ) : (
          <ul className="flex flex-col divide-y divide-line">
            {(tasks.data ?? []).map((task) => (
              <TaskRow key={task.id} task={task} />
            ))}
          </ul>
        )}
      </Card>
    </div>
  )
}
