import { create } from 'zustand'
import type { AnalysisProfile } from '@/api/types'

/** The analysis to start once the user answered the face-recognition question. */
export interface PendingAnalysis {
  sessionId: number
  profile: AnalysisProfile
  photoIds?: number[]
}

/**
 * Face-recognition consent dialog (docs/02 §8): opened before the first analysis while
 * `faces.consented` is false, or when face recognition is switched on in the settings.
 */
interface FaceConsentState {
  open: boolean
  /** `null`: asked from the settings toggle, nothing to start afterwards. */
  analysis: PendingAnalysis | null
  ask: (analysis: PendingAnalysis | null) => void
  close: () => void
}

export const useFaceConsent = create<FaceConsentState>((set) => ({
  open: false,
  analysis: null,
  ask: (analysis) => set({ open: true, analysis }),
  close: () => set({ open: false, analysis: null }),
}))
