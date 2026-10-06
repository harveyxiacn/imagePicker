import { create } from 'zustand'
import type { Stroke } from '@/api/types'
import { clampRadius, DEFAULT_RADIUS } from '@/lib/strokes'

/** Transient UI state of the 修复 section: brush tool, strokes waiting to be applied, bystander hover. */
interface RepairState {
  brush: boolean
  radius: number
  strokes: Stroke[]
  /** hovering "消除路人": the canvas outlines the detected bystanders */
  hoverBystanders: boolean
  setBrush: (b: boolean) => void
  setRadius: (r: number) => void
  addStroke: (s: Stroke) => void
  undoStroke: () => void
  clearStrokes: () => void
  setHoverBystanders: (b: boolean) => void
  reset: () => void
}

export const useRepair = create<RepairState>((set) => ({
  brush: false,
  radius: DEFAULT_RADIUS,
  strokes: [],
  hoverBystanders: false,
  setBrush: (brush) => set((s) => ({ brush, strokes: brush ? s.strokes : [] })),
  setRadius: (r) => set({ radius: clampRadius(r) }),
  addStroke: (stroke) => set((s) => ({ strokes: [...s.strokes, stroke] })),
  undoStroke: () => set((s) => ({ strokes: s.strokes.slice(0, -1) })),
  clearStrokes: () => set({ strokes: [] }),
  setHoverBystanders: (hoverBystanders) => set({ hoverBystanders }),
  reset: () => set({ brush: false, strokes: [], hoverBystanders: false }),
}))
