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
  updateTask: (t: TaskEvent) => void
  dismissTask: (id: string) => void
}

let seq = 0

export const useToasts = create<ToastState>((set, get) => ({
  toasts: [],
  tasks: {},
  push: (kind, text, ttl = 4000, actions) => {
    const id = `t${++seq}`
    set((s) => ({ toasts: [...s.toasts, { id, kind, text, actions }] }))
    if (ttl > 0) setTimeout(() => get().dismiss(id), ttl)
  },
  dismiss: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
  updateTask: (t) => {
    set((s) => ({ tasks: { ...s.tasks, [t.task_id]: t } }))
    if (t.state !== 'running') setTimeout(() => get().dismissTask(t.task_id), t.state === 'failed' ? 8000 : 5000)
  },
  dismissTask: (id) =>
    set((s) => {
      const tasks = { ...s.tasks }
      delete tasks[id]
      return { tasks }
    }),
}))
