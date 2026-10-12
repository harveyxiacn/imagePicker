/**
 * Mock task history and cancellation (docs/api-contract-m5.md F): `GET /api/tasks`, `POST /api/tasks/:id/cancel`.
 * The mock M5 tasks and exports record themselves here and poll `isCancelled` between their steps.
 */
import { http, HttpResponse } from 'msw'
import type { TaskRecord, TaskStatus } from '@/api/types'

const KEEP = 200
const records: TaskRecord[] = []
const cancelled = new Set<string>()

const err = (status: number, code: string, message: string) => HttpResponse.json({ error: { code, message } }, { status })

/** A task that just started. */
export function trackTask(id: string, kind: string, params: Record<string, unknown>, total: number): void {
  const now = Date.now()
  records.unshift({ id, kind, status: 'running', params, done: 0, total, error: null, created_at: now, updated_at: now, finished_at: null, cancellable: true })
  while (records.length > KEEP) records.pop()
}

export function taskProgress(id: string, done: number, total: number): void {
  const r = records.find((x) => x.id === id)
  if (r) Object.assign(r, { done, total, updated_at: Date.now() })
}

export function endTask(id: string, status: Exclude<TaskStatus, 'running' | 'cancelling'>, error: string | null = null): void {
  const r = records.find((x) => x.id === id)
  const now = Date.now()
  if (r) Object.assign(r, { status, error, updated_at: now, finished_at: now, cancellable: false, ...(status === 'done' ? { done: r.total } : {}) })
  cancelled.delete(id)
}

/** The user asked to stop the task (it ends `cancelled` at its next step). */
export const isCancelled = (id: string): boolean => cancelled.has(id)

// a little history so the view is not empty on first open
{
  const t0 = Date.now() - 3_600_000
  records.push(
    { id: 'enhance-0', kind: 'enhance', status: 'interrupted', params: { photo_id: 3, op: 'denoise', strength: 0.6 }, done: 0, total: 1, error: null, created_at: t0, updated_at: t0 + 4000, finished_at: t0 + 4000, cancellable: false },
    { id: 'export-0', kind: 'export', status: 'done', params: { dest: 'D:\\Export', count: 12 }, done: 12, total: 12, error: null, created_at: t0 - 600_000, updated_at: t0 - 590_000, finished_at: t0 - 590_000, cancellable: false },
  )
}

export const taskHandlers = [
  http.get('/api/tasks', ({ request }) => {
    const raw = new URL(request.url).searchParams.get('limit')
    const limit = raw === null ? 50 : Number(raw)
    if (!Number.isInteger(limit) || limit < 1 || limit > KEEP) return err(400, 'bad_request', `limit must be within 1..${KEEP}`)
    const tasks = records.slice(0, limit).map((r) => (r.status === 'running' && cancelled.has(r.id) ? { ...r, status: 'cancelling' as const, cancellable: false } : { ...r }))
    return HttpResponse.json({ tasks })
  }),
  http.post('/api/tasks/:id/cancel', ({ params }) => {
    const id = String(params.id)
    const r = records.find((x) => x.id === id)
    if (!r) return err(404, 'not_found', `task ${id} not found`)
    if (r.status !== 'running') return err(409, 'conflict', `task ${id} is not running (${r.status})`)
    cancelled.add(id)
    return HttpResponse.json({ task: { ...r, status: 'cancelling', cancellable: false } }, { status: 202 })
  }),
]
