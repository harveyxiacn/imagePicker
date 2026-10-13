import { describe, expect, it } from 'vitest'
import type { ModelUsage } from '@/api/types'
import { effectiveUsage, licenceBadge, licenceWarnings } from './licence'

const m = (usage: ModelUsage | undefined, noncommercial = false) => ({ usage: usage as ModelUsage, noncommercial })

describe('licence badge logic', () => {
  it('commercial-ok is plain, everything else is highlighted', () => {
    expect(licenceBadge(m('commercial-ok'))).toEqual({ usage: 'commercial-ok', level: 'ok', needsAck: false })
    expect(licenceBadge(m('unknown'))).toEqual({ usage: 'unknown', level: 'warn', needsAck: false })
    expect(licenceBadge(m('research-only'))).toEqual({ usage: 'research-only', level: 'warn', needsAck: true })
    expect(licenceBadge(m('non-commercial'))).toEqual({ usage: 'non-commercial', level: 'warn', needsAck: true })
  })

  it('the legacy noncommercial flag wins over a looser usage', () => {
    expect(effectiveUsage(m('commercial-ok', true))).toBe('non-commercial')
    expect(licenceBadge(m('unknown', true)).needsAck).toBe(true)
  })

  it('a missing or unrecognised flag is unknown, never ok', () => {
    expect(effectiveUsage(m(undefined))).toBe('unknown')
    expect(effectiveUsage(m('free-for-all' as ModelUsage))).toBe('unknown')
  })

  it('lists the distinct warnings worst first', () => {
    expect(licenceWarnings([m('commercial-ok'), m('commercial-ok')])).toEqual([])
    expect(licenceWarnings([m('unknown'), m('commercial-ok'), m('non-commercial'), m('unknown')])).toEqual(['non-commercial', 'unknown'])
    expect(licenceWarnings([m('unknown'), m('research-only')])).toEqual(['research-only', 'unknown'])
  })
})
