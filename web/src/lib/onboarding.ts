/** First-run onboarding (doc 04 section 7): hardware card -> three coach marks -> done. Pure state machine. */

export const COACH_MARKS = ['analyze', 'stacks', 'rating'] as const
export type CoachMark = (typeof COACH_MARKS)[number]

export type OnboardingStep = 'hardware' | 'coach' | 'finished'

export interface OnboardingState {
  step: OnboardingStep
  /** index into COACH_MARKS while `step === 'coach'` */
  coach: number
  /** the user chose to download the AI components */
  downloading: boolean
}

export const initialOnboarding: OnboardingState = { step: 'hardware', coach: 0, downloading: false }

export type OnboardingAction =
  | { type: 'download' }
  | { type: 'basic' }
  | { type: 'next' }
  | { type: 'back' }
  | { type: 'skipTour' }
  /** server says `first_run:false` (done on another device / earlier) */
  | { type: 'alreadyDone' }
  | { type: 'reset' }

export function onboardingReducer(s: OnboardingState, a: OnboardingAction): OnboardingState {
  switch (a.type) {
    case 'download':
      return s.step === 'hardware' ? { step: 'coach', coach: 0, downloading: true } : s
    case 'basic':
      return s.step === 'hardware' ? { step: 'coach', coach: 0, downloading: false } : s
    case 'next':
      if (s.step !== 'coach') return s
      return s.coach + 1 >= COACH_MARKS.length ? { ...s, step: 'finished' } : { ...s, coach: s.coach + 1 }
    case 'back':
      return s.step === 'coach' && s.coach > 0 ? { ...s, coach: s.coach - 1 } : s
    case 'skipTour':
      return s.step === 'finished' ? s : { ...s, step: 'finished' }
    case 'alreadyDone':
      return s.step === 'finished' ? s : { ...s, step: 'finished' }
    case 'reset':
      return initialOnboarding
  }
}

/** `POST /api/onboarding/done` is sent exactly once, on the transition into `finished`. */
export const shouldPostDone = (prev: OnboardingState, next: OnboardingState): boolean => prev.step !== 'finished' && next.step === 'finished'

/** Which overlay to show: the hardware card (anywhere) or a coach mark (only inside a library, where the targets exist). */
export function visibleOverlay(s: OnboardingState, firstRun: boolean, inLibrary: boolean): 'card' | CoachMark | null {
  if (!firstRun || s.step === 'finished') return null
  if (s.step === 'hardware') return 'card'
  return inLibrary ? COACH_MARKS[s.coach] : null
}
