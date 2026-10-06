import { create } from 'zustand'
import type { EditStack, Photo, PhotoEditable, PhotoPatchBody } from '@/api/types'

export type EditableKey = keyof PhotoEditable
export type EditablePatch = Partial<PhotoEditable>

export interface Change {
  id: number
  before: EditablePatch
  after: EditablePatch
}

/** Before/after edit stacks of one photo (M3: undo = PUT the previous stack). */
export interface EditChange {
  photoId: number
  before: EditStack
  after: EditStack
}

export interface HistoryEntry {
  label: string
  /** rating / flag / label changes (empty for pure edit entries) */
  changes: Change[]
  /** edit-stack changes; one entry may cover several photos (paste / sync) */
  edits?: EditChange[]
  /** commits with the same group key inside `COALESCE_MS` merge into one undo step (wheel / arrow fine-tune) */
  group?: string
  /** push time (ms), used for coalescing */
  at?: number
}

export const COALESCE_MS = 900

/** Merge `next` into `prev` when both are quick successive commits of the same control on the same photo. */
export function coalesceEntry(prev: HistoryEntry | undefined, next: HistoryEntry, windowMs = COALESCE_MS): HistoryEntry | null {
  if (!prev || !next.group || prev.group !== next.group) return null
  if ((next.at ?? 0) - (prev.at ?? 0) > windowMs) return null
  if (prev.edits?.length !== 1 || next.edits?.length !== 1) return null
  if (prev.edits[0].photoId !== next.edits[0].photoId) return null
  return { ...prev, edits: [{ ...prev.edits[0], after: next.edits[0].after }], at: next.at }
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
  /** Remove the entry holding exactly these changes (rollback of a failed write). */
  drop: (changes: Change[]) => void
}

export const useHistory = create<HistoryState>((set, get) => ({
  undoStack: [],
  redoStack: [],
  push: (e) =>
    set((s) => {
      const merged = coalesceEntry(s.undoStack[s.undoStack.length - 1], e)
      const undoStack = merged ? [...s.undoStack.slice(0, -1), merged] : [...s.undoStack, e].slice(-LIMIT)
      return { undoStack, redoStack: [] }
    }),
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
  drop: (changes) => set((s) => ({ undoStack: s.undoStack.filter((e) => e.changes !== changes) })),
}))
