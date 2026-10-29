import { create } from 'zustand'
import type { TaskEvent } from '@/lib/cache'

export interface Toast {
  id: string
  kind: 'info' | 'error' | 'success'
  text: string
  /** optional action buttons (e.g. XMP conflict resolution); the toast closes after an action runs */
  actions?: { label: string; onClick: () => void }[]
}

interface ToastState {
  toasts: Toast[]
  tasks: Record<string, TaskEvent>
  push: (kind: Toast['kind'], text: string, ttl?: number, actions?: Toast['actions']) => void
  dismiss: (id: string) => void
  /** hold / restart the auto-dismiss countdown (hover and keyboard focus on the toast) */
  pause: (id: string) => void
  resume: (id: string) => void
  updateTask: (t: TaskEvent) => void
  dismissTask: (id: string) => void
}

let seq = 0

/** Toasts with an action button (Undo / Retry) stay at least this long. */
export const ACTION_TOAST_MIN_MS = 10000

/** Auto-dismiss bookkeeping per toast: timer handle, remaining ms and when the current countdown started. */
const timers = new Map<string, { handle: ReturnType<typeof setTimeout> | null; left: number; since: number }>()

export const useToasts = create<ToastState>((set, get) => ({
  toasts: [],
  tasks: {},
  push: (kind, text, ttl = 4000, actions) => {
    const id = `t${++seq}`
    set((s) => ({ toasts: [...s.toasts, { id, kind, text, actions }] }))
    const life = ttl > 0 && actions?.length ? Math.max(ttl, ACTION_TOAST_MIN_MS) : ttl
    if (life > 0) {
      timers.set(id, { handle: setTimeout(() => get().dismiss(id), life), left: life, since: Date.now() })
    }
  },
  dismiss: (id) => {
    const tm = timers.get(id)
    if (tm?.handle) clearTimeout(tm.handle)
    timers.delete(id)
    set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) }))
  },
  pause: (id) => {
    const tm = timers.get(id)
    if (!tm || !tm.handle) return
    clearTimeout(tm.handle)
    tm.handle = null
    tm.left = Math.max(0, tm.left - (Date.now() - tm.since))
  },
  resume: (id) => {
    const tm = timers.get(id)
    if (!tm || tm.handle) return
    tm.since = Date.now()
    // restart with at least a short grace period so the toast does not vanish the instant the pointer leaves
    tm.left = Math.max(tm.left, 2000)
    tm.handle = setTimeout(() => get().dismiss(id), tm.left)
  },
  updateTask: (t) => {
    set((s) => ({ tasks: { ...s.tasks, [t.task_id]: t } }))
    if (t.state !== 'running' && t.state !== 'paused') setTimeout(() => get().dismissTask(t.task_id), t.state === 'failed' ? 8000 : 5000)
  },
  dismissTask: (id) =>
    set((s) => {
      const tasks = { ...s.tasks }
      delete tasks[id]
      return { tasks }
    }),
}))
