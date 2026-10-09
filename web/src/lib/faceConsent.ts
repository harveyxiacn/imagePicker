/** Face-recognition consent (docs/02 §8): face features are sensitive personal data, so nothing about faces is computed before the user agreed. */
import type { QueryClient } from '@tanstack/react-query'
import { api } from '@/api/client'
import type { Settings, SettingsPatch } from '@/api/types'
import { qk } from './cache'

/** The purpose explanation has to be shown first: face recognition is on but not agreed to yet. */
export function needsFaceConsent(s: Pick<Settings, 'faces'> | null | undefined): boolean {
  return !!s && s.faces.enabled && !s.faces.consented
}

/** Settings patch for the user's answer: agreeing keeps face recognition on, declining switches it off. */
export function faceConsentPatch(agree: boolean): SettingsPatch {
  return agree ? { faces: { enabled: true, consented: true } } : { faces: { enabled: false } }
}

/** Whether the next analysis has to ask first (settings from the cache, fetched when missing; `false` when they cannot be read). */
export async function faceConsentPending(qc: QueryClient): Promise<boolean> {
  try {
    const s = qc.getQueryData<Settings>(qk.settings) ?? (await qc.fetchQuery({ queryKey: qk.settings, queryFn: api.settings, staleTime: 30_000 }))
    return needsFaceConsent(s)
  } catch {
    // guests cannot read the settings (and cannot start an analysis either); the server never computes faces without consent anyway
    return false
  }
}
