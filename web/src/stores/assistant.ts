import { create } from 'zustand'
import { assistantReducer, initialAssistantState, type AssistantAction, type AssistantState } from '@/lib/assistant'

interface AssistantStore extends AssistantState {
  open: boolean
  setOpen: (b: boolean) => void
  toggle: () => void
  dispatch: (a: AssistantAction) => void
}

/** Drawer visibility + transcript (reducer in lib/assistant.ts). Transcript survives navigation, not reloads. */
export const useAssistant = create<AssistantStore>((set) => ({
  ...initialAssistantState,
  open: false,
  setOpen: (open) => set({ open }),
  toggle: () => set((s) => ({ open: !s.open })),
  dispatch: (a) => set((s) => assistantReducer(s, a)),
}))
