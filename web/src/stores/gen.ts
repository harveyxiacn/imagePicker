import { create } from 'zustand'
import type { BestTakeResult, EditStack } from '@/api/types'

export type GenKind = 'besttake' | 'inpaint' | 'enhance'
export const GEN_KINDS: readonly string[] = ['besttake', 'inpaint', 'enhance']
export const isGenKind = (k: string): k is GenKind => GEN_KINDS.includes(k)

/** A running generative task (best take / inpaint / enhance) and what is needed to turn its result into ONE undo step. */
export interface GenTask {
  taskId: string
  kind: GenKind
  /** photo whose stack the task edits (for auto best take: the burst's current base, the real base arrives with `besttake.done`) */
  photoId: number
  label: string
  /** stack of `photoId` before the request (history `before`) */
  before: EditStack
  /** auto best take: stacks of every burst photo before the request, keyed by photo id */
  snapshots?: Record<number, EditStack>
  /** best take: base faces being processed (progress overlay on the face) */
  baseFaceIds: number[]
  done: number
  total: number
  state: 'running' | 'done' | 'failed'
  startedAt: number
}

export interface BestTakeOutcome extends BestTakeResult {
  photoId: number
  at: number
}

interface GenState {
  tasks: Record<string, GenTask>
  /** latest best-take result per base face (warnings / failure reason shown in the editor) */
  results: Record<number, BestTakeOutcome>
  /** set when `besttake/auto` finished: the editor switches to this base photo */
  autoBase: { photoId: number; nonce: number } | null
  begin: (t: GenTask) => void
  progress: (taskId: string, done: number, total: number) => void
  remove: (taskId: string) => GenTask | undefined
  setResults: (photoId: number, results: BestTakeResult[]) => void
  clearResult: (baseFaceId: number) => void
  setAutoBase: (photoId: number) => void
}

export const useGen = create<GenState>((set, get) => ({
  tasks: {},
  results: {},
  autoBase: null,
  begin: (t) => set((s) => ({ tasks: { ...s.tasks, [t.taskId]: t } })),
  progress: (taskId, done, total) =>
    set((s) => {
      const t = s.tasks[taskId]
      return t ? { tasks: { ...s.tasks, [taskId]: { ...t, done, total } } } : s
    }),
  remove: (taskId) => {
    const t = get().tasks[taskId]
    if (!t) return undefined
    set((s) => {
      const tasks = { ...s.tasks }
      delete tasks[taskId]
      return { tasks }
    })
    return t
  },
  setResults: (photoId, results) =>
    set((s) => {
      const next = { ...s.results }
      const at = Date.now()
      for (const r of results) next[r.base_face_id] = { ...r, photoId, at }
      return { results: next }
    }),
  clearResult: (baseFaceId) =>
    set((s) => {
      const next = { ...s.results }
      delete next[baseFaceId]
      return { results: next }
    }),
  setAutoBase: (photoId) => set({ autoBase: { photoId, nonce: Date.now() } }),
}))

/** Running tasks of one photo (the best-take editor also matches any task of its burst via `baseFaceIds`). */
export const runningTasks = (tasks: Record<string, GenTask>): GenTask[] => Object.values(tasks).filter((t) => t.state === 'running')
