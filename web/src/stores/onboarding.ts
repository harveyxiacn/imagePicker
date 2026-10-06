import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import { initialOnboarding, onboardingReducer, type OnboardingAction, type OnboardingState } from '@/lib/onboarding'

interface OnboardingStore extends OnboardingState {
  /** `POST /api/onboarding/done` has been sent (or is not needed) */
  dispatch: (a: OnboardingAction) => OnboardingState
}

/** Local progress through the first-run flow (the server only knows `first_run`). */
export const useOnboardingUi = create<OnboardingStore>()(
  persist(
    (set, get) => ({
      ...initialOnboarding,
      dispatch: (a) => {
        const next = onboardingReducer(get(), a)
        set({ step: next.step, coach: next.coach, downloading: next.downloading })
        return next
      },
    }),
    { name: 'imagepicker.onboarding', partialize: (s) => ({ step: s.step, coach: s.coach, downloading: s.downloading }) },
  ),
)
