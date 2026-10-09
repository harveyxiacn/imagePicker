import { describe, expect, it } from 'vitest'
import { DEFAULT_SETTINGS, mergeSettings } from './settingsForm'
import { faceConsentPatch, needsFaceConsent } from './faceConsent'

describe('face recognition consent', () => {
  it('is asked for while face recognition is on and not agreed to', () => {
    expect(DEFAULT_SETTINGS.faces).toEqual({ enabled: true, consented: false })
    expect(needsFaceConsent(DEFAULT_SETTINGS)).toBe(true)
    expect(needsFaceConsent(mergeSettings(DEFAULT_SETTINGS, { faces: { consented: true } }))).toBe(false)
    expect(needsFaceConsent(mergeSettings(DEFAULT_SETTINGS, { faces: { enabled: false } }))).toBe(false)
    expect(needsFaceConsent(undefined)).toBe(false)
  })

  it('agreeing keeps faces on, declining switches them off', () => {
    const agreed = mergeSettings(DEFAULT_SETTINGS, faceConsentPatch(true))
    expect(agreed.faces).toEqual({ enabled: true, consented: true })
    const declined = mergeSettings(DEFAULT_SETTINGS, faceConsentPatch(false))
    expect(declined.faces).toEqual({ enabled: false, consented: false })
    expect(needsFaceConsent(declined)).toBe(false)
  })
})
