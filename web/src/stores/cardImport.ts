import { create } from 'zustand'
import type { TaskEvent } from '@/lib/cache'

interface CardImportState {
  /** Latest `task.progress` event per `card_import` task (the Home dialog shows the live count). */
  events: Record<string, TaskEvent>
  update: (e: TaskEvent) => void
  clear: (id: string) => void
}

export const useCardImport = create<CardImportState>((set) => ({
  events: {},
  update: (e) => set((s) => ({ events: { ...s.events, [e.task_id]: e } })),
  clear: (id) =>
    set((s) => {
      const events = { ...s.events }
      delete events[id]
      return { events }
    }),
}))
