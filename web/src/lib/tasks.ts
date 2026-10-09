/**
 * Task history and cancellation (docs/api-contract-m5.md F): `GET /api/tasks` lists recorded tasks (generation,
 * export, analysis), `POST /api/tasks/{id}/cancel` stops a running one.
 */
import type { QueryClient } from '@tanstack/react-query'
import { api, ApiError } from '@/api/client'
import type { TaskRecord, TaskStatus } from '@/api/types'
import { useToasts } from '@/stores/toasts'
import { qk } from './cache'

/** Error / `*.done` reason of a task stopped by the user. */
export const CANCELLED = 'cancelled'

export const TASK_KINDS = ['besttake', 'inpaint', 'enhance', 'export', 'analysis'] as const
export const TASK_STATUSES: readonly TaskStatus[] = ['running', 'cancelling', 'done', 'failed', 'cancelled', 'interrupted']

/** Still going on the server (the history polls while any task is live). */
export const isLive = (s: TaskStatus): boolean => s === 'running' || s === 'cancelling'

/** 0..100; a finished task is full, a task without a known total shows nothing. */
export function taskPercent(t: Pick<TaskRecord, 'done' | 'total' | 'status'>): number {
  if (t.status === 'done') return 100
  if (t.total <= 0) return 0
  return Math.min(100, Math.max(0, Math.round((t.done / t.total) * 100)))
}

/** i18n key + options describing what a task worked on (from its `params`), or null when unknown. */
export function taskDetail(t: Pick<TaskRecord, 'kind' | 'params'>): { key: string; opts: Record<string, unknown> } | null {
  const p = t.params ?? {}
  const num = (k: string) => (typeof p[k] === 'number' ? (p[k] as number) : null)
  const str = (k: string) => (typeof p[k] === 'string' ? (p[k] as string) : null)
  switch (t.kind) {
    case 'besttake':
    case 'inpaint':
      return num('photo_id') !== null ? { key: 'tasks.detail_photo', opts: { id: num('photo_id') } } : null
    case 'enhance': {
      const id = num('photo_id')
      if (id === null) return null
      const op = str('op')
      return op ? { key: 'tasks.detail_photoOp', opts: { id, op } } : { key: 'tasks.detail_photo', opts: { id } }
    }
    case 'export': {
      const n = num('count')
      const dest = str('dest')
      return n !== null ? { key: 'tasks.detail_export', opts: { count: n, dest: dest ?? '' } } : null
    }
    case 'analysis':
      return num('session_id') !== null ? { key: 'tasks.detail_session', opts: { id: num('session_id'), count: num('count') ?? 0 } } : null
    default:
      return null
  }
}

/**
 * Asks the server to stop a task. Returns true when the request was accepted. A task that already ended (409) or
 * vanished (404) is not an error worth a toast: the history just refreshes.
 */
export async function cancelTask(qc: QueryClient, id: string): Promise<boolean> {
  try {
    await api.cancelTask(id)
    return true
  } catch (err) {
    if (!(err instanceof ApiError && (err.status === 409 || err.status === 404))) {
      useToasts.getState().push('error', err instanceof Error ? err.message : String(err))
    }
    return false
  } finally {
    void qc.invalidateQueries({ queryKey: qk.tasks })
  }
}
