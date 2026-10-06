import { describe, expect, it } from 'vitest'
import { COACH_MARKS, initialOnboarding, onboardingReducer, shouldPostDone, visibleOverlay, type OnboardingAction, type OnboardingState } from './onboarding'

const run = (...a: OnboardingAction[]): OnboardingState => a.reduce(onboardingReducer, initialOnboarding)

describe('onboarding state machine', () => {
  it('starts on the hardware card', () => {
    expect(initialOnboarding.step).toBe('hardware')
    expect(visibleOverlay(initialOnboarding, true, false)).toBe('card')
  })

  it('download and basic both lead to the first coach mark, remembering the choice', () => {
    expect(run({ type: 'download' })).toEqual({ step: 'coach', coach: 0, downloading: true })
    expect(run({ type: 'basic' })).toEqual({ step: 'coach', coach: 0, downloading: false })
  })

  it('walks the three coach marks and finishes', () => {
    let s = run({ type: 'basic' })
    const seen: string[] = []
    for (let i = 0; i < COACH_MARKS.length; i++) {
      seen.push(visibleOverlay(s, true, true) as string)
      s = onboardingReducer(s, { type: 'next' })
    }
    expect(seen).toEqual(['analyze', 'stacks', 'rating'])
    expect(s.step).toBe('finished')
    expect(visibleOverlay(s, true, true)).toBeNull()
  })

  it('back never goes below the first mark; actions are ignored in the wrong step', () => {
    expect(run({ type: 'basic' }, { type: 'back' }).coach).toBe(0)
    expect(run({ type: 'next' })).toEqual(initialOnboarding)
    expect(run({ type: 'basic' }, { type: 'download' }).downloading).toBe(false)
  })

  it('coach marks wait for a library page', () => {
    const s = run({ type: 'basic' })
    expect(visibleOverlay(s, true, false)).toBeNull()
    expect(visibleOverlay(s, true, true)).toBe('analyze')
  })

  it('nothing shows once the server says first_run is over', () => {
    expect(visibleOverlay(initialOnboarding, false, true)).toBeNull()
    expect(run({ type: 'alreadyDone' }).step).toBe('finished')
  })

  it('skip tour finishes; POST done fires exactly once', () => {
    const a = run({ type: 'basic' })
    const b = onboardingReducer(a, { type: 'skipTour' })
    expect(shouldPostDone(a, b)).toBe(true)
    expect(shouldPostDone(b, onboardingReducer(b, { type: 'skipTour' }))).toBe(false)
    expect(shouldPostDone(initialOnboarding, a)).toBe(false)
  })
})
