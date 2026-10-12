import { create } from 'zustand'
import type { AnalysisProfile } from '@/api/types'

export interface ConsentRequest {
  sessionId: number
  profile: AnalysisProfile
  photoIds?: number[]
  /** Model ids the server reported as missing (409 models_missing). */
  models: string[]
  /** When set, called after the download instead of retrying an analysis (M3 AI masks). */
  onReady?: () => void
}

interface AnalysisUiState {
  /** Profile last chosen in the analyze menu. */
  profile: AnalysisProfile
  menuOpen: boolean
  consent: ConsentRequest | null
  /** task id of the running /api/models/ensure download, if any */
  ensureTaskId: string | null
  ensureError: string | null
  /** sessions whose pause was requested and has not taken effect yet (the batch in flight finishes first) */
  pausing: Record<number, boolean>
  setProfile: (p: AnalysisProfile) => void
  setMenuOpen: (b: boolean) => void
  setConsent: (c: ConsentRequest | null) => void
  setEnsure: (taskId: string | null, error?: string | null) => void
  setPausing: (sessionId: number, pausing: boolean) => void
}

export const useAnalysisUi = create<AnalysisUiState>((set) => ({
  profile: 'standard',
  menuOpen: false,
  consent: null,
  ensureTaskId: null,
  ensureError: null,
  pausing: {},
  setProfile: (profile) => set({ profile }),
  setMenuOpen: (menuOpen) => set({ menuOpen }),
  setConsent: (consent) => set({ consent, ensureTaskId: null, ensureError: null }),
  setEnsure: (ensureTaskId, ensureError = null) => set({ ensureTaskId, ensureError }),
  setPausing: (sessionId, pausing) =>
    set((s) => {
      if (!!s.pausing[sessionId] === pausing) return s
      const next = { ...s.pausing }
      if (pausing) next[sessionId] = true
      else delete next[sessionId]
      return { pausing: next }
    }),
}))
