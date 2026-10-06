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
  setProfile: (p: AnalysisProfile) => void
  setMenuOpen: (b: boolean) => void
  setConsent: (c: ConsentRequest | null) => void
  setEnsure: (taskId: string | null, error?: string | null) => void
}

export const useAnalysisUi = create<AnalysisUiState>((set) => ({
  profile: 'standard',
  menuOpen: false,
  consent: null,
  ensureTaskId: null,
  ensureError: null,
  setProfile: (profile) => set({ profile }),
  setMenuOpen: (menuOpen) => set({ menuOpen }),
  setConsent: (consent) => set({ consent, ensureTaskId: null, ensureError: null }),
  setEnsure: (ensureTaskId, ensureError = null) => set({ ensureTaskId, ensureError }),
}))
