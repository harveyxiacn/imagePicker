import { create } from 'zustand'
import type { Photo, PhotoEditable, PhotoPatchBody } from '@/api/types'

export type EditableKey = keyof PhotoEditable
export type EditablePatch = Partial<PhotoEditable>

export interface Change {
  id: number
  before: EditablePatch
  after: EditablePatch
}

export interface HistoryEntry {
  label: string
  changes: Change[]
}

const KEYS: EditableKey[] = ['user_rating', 'flag', 'color_label']

/** Diff `patch` against current photo values; unchanged photos/fields are skipped. */
export function computeChanges(
  photos: Pick<Photo, 'id' | EditableKey>[],
  patch: EditablePatch,
): Change[] {
  const out: Change[] = []
  for (const p of photos) {
    const before: Record<string, unknown> = {}
    const after: Record<string, unknown> = {}
    for (const k of KEYS) {
      if (k in patch && patch[k] !== p[k]) {
        before[k] = p[k]
        after[k] = patch[k]
      }
    }
    if (Object.keys(after).length) out.push({ id: p.id, before: before as EditablePatch, after: after as EditablePatch })
  }
  return out
}

/** Group per-photo changes by identical patch so each group is one PATCH request. */
export function groupPatches(changes: Change[], side: 'before' | 'after'): PhotoPatchBody[] {
  const groups = new Map<string, PhotoPatchBody>()
  for (const c of changes) {
    const patch = c[side]
    const key = JSON.stringify(patch)
    const g = groups.get(key)
    if (g) g.ids.push(c.id)
    else groups.set(key, { ids: [c.id], ...patch })
  }
  return [...groups.values()]
}

const LIMIT = 200

interface HistoryState {
  undoStack: HistoryEntry[]
  redoStack: HistoryEntry[]
  push: (e: HistoryEntry) => void
  /** Pops the newest undo entry (moving it to redo). */
  undo: () => HistoryEntry | undefined
  /** Pops the newest redo entry (moving it back to undo). */
  redo: () => HistoryEntry | undefined
  clear: () => void
}

export const useHistory = create<HistoryState>((set, get) => ({
  undoStack: [],
  redoStack: [],
  push: (e) =>
    set((s) => ({ undoStack: [...s.undoStack, e].slice(-LIMIT), redoStack: [] })),
  undo: () => {
    const { undoStack, redoStack } = get()
    const e = undoStack[undoStack.length - 1]
    if (!e) return undefined
    set({ undoStack: undoStack.slice(0, -1), redoStack: [...redoStack, e] })
    return e
  },
  redo: () => {
    const { undoStack, redoStack } = get()
    const e = redoStack[redoStack.length - 1]
    if (!e) return undefined
    set({ redoStack: redoStack.slice(0, -1), undoStack: [...undoStack, e] })
    return e
  },
  clear: () => set({ undoStack: [], redoStack: [] }),
}))
