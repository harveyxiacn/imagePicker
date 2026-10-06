import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import type { EditSection, EditStack } from '@/api/types'
import { emptyStack, type AdjustDiff } from '@/lib/edit'

export type CompareMode = 'off' | 'original' | 'split'
export type SaveState = 'idle' | 'saving' | 'error'
/** Panels of the right column that can be collapsed. */
export type PanelId = 'ai' | 'basic' | 'curves' | 'hsl' | 'grading' | 'local' | 'crop' | 'presets' | 'sharpen'

export interface Clipboard {
  stack: EditStack
  sections: EditSection[]
  fromId: number
}

export interface AutoApplied {
  mode: string
  diff: AdjustDiff[]
  /** key bumped on every apply so the diff animation replays */
  nonce: number
}

interface EditState {
  photoId: number | null
  /** live stack: what the preview renders, may be mid-drag */
  stack: EditStack
  /** last committed stack (the `before` of the next history entry) */
  committed: EditStack
  loaded: boolean
  dragging: boolean
  compare: CompareMode
  splitPos: number
  cropMode: boolean
  maskOverlay: boolean
  activeLocal: number | null
  /** Alt held while dragging a slider (clipping preview stub). */
  clipping: boolean
  /** slider keys highlighted after an AI apply */
  flash: string[]
  autoApplied: AutoApplied | null
  clipboard: Clipboard | null
  saveState: SaveState
  historyOpen: boolean
  copyDialog: boolean
  /** .cube files imported this browser session / earlier (ids usable as `{type:"lut",file:id}`) */
  importedLuts: { id: string; name: string }[]
  collapsed: Partial<Record<PanelId, boolean>>

  load: (photoId: number, stack: EditStack) => void
  /** Replace live + committed (undo/redo/reset applied from outside). */
  reset: (photoId: number, stack: EditStack) => void
  live: (stack: EditStack) => void
  markCommitted: () => void
  setDragging: (b: boolean) => void
  setCompare: (m: CompareMode) => void
  setSplitPos: (n: number) => void
  setCropMode: (b: boolean) => void
  setMaskOverlay: (b: boolean) => void
  setActiveLocal: (i: number | null) => void
  setClipping: (b: boolean) => void
  setFlash: (keys: string[]) => void
  setAutoApplied: (a: AutoApplied | null) => void
  setClipboard: (c: Clipboard | null) => void
  setSaveState: (s: SaveState) => void
  setHistoryOpen: (b: boolean) => void
  setCopyDialog: (b: boolean) => void
  addLut: (l: { id: string; name: string }) => void
  togglePanel: (id: PanelId) => void
}

export const useEdit = create<EditState>()(
  persist(
    (set) => ({
      photoId: null,
      stack: emptyStack(),
      committed: emptyStack(),
      loaded: false,
      dragging: false,
      compare: 'off',
      splitPos: 0.5,
      cropMode: false,
      maskOverlay: false,
      activeLocal: null,
      clipping: false,
      flash: [],
      autoApplied: null,
      clipboard: null,
      saveState: 'idle',
      historyOpen: false,
      copyDialog: false,
      importedLuts: [],
      collapsed: {},

      load: (photoId, stack) =>
        set({
          photoId,
          stack,
          committed: stack,
          loaded: true,
          dragging: false,
          compare: 'off',
          cropMode: false,
          maskOverlay: false,
          activeLocal: null,
          clipping: false,
          flash: [],
          autoApplied: null,
        }),
      reset: (photoId, stack) => set((s) => (s.photoId === photoId ? { stack, committed: stack, loaded: true } : s)),
      live: (stack) => set({ stack }),
      markCommitted: () => set((s) => ({ committed: s.stack })),
      setDragging: (dragging) => set({ dragging }),
      setCompare: (compare) => set({ compare }),
      setSplitPos: (splitPos) => set({ splitPos: Math.min(0.98, Math.max(0.02, splitPos)) }),
      setCropMode: (cropMode) => set({ cropMode, ...(cropMode ? { compare: 'off' as const, maskOverlay: false } : {}) }),
      setMaskOverlay: (maskOverlay) => set({ maskOverlay }),
      setActiveLocal: (activeLocal) => set({ activeLocal }),
      setClipping: (clipping) => set({ clipping }),
      setFlash: (flash) => set({ flash }),
      setAutoApplied: (autoApplied) => set({ autoApplied }),
      setClipboard: (clipboard) => set({ clipboard }),
      setSaveState: (saveState) => set({ saveState }),
      setHistoryOpen: (historyOpen) => set({ historyOpen }),
      setCopyDialog: (copyDialog) => set({ copyDialog }),
      addLut: (l) => set((s) => ({ importedLuts: s.importedLuts.some((x) => x.id === l.id) ? s.importedLuts : [...s.importedLuts, l] })),
      togglePanel: (id) => set((s) => ({ collapsed: { ...s.collapsed, [id]: !s.collapsed[id] } })),
    }),
    {
      name: 'imagepicker.edit',
      partialize: (s) => ({ importedLuts: s.importedLuts, collapsed: s.collapsed, clipboard: s.clipboard }),
    },
  ),
)
