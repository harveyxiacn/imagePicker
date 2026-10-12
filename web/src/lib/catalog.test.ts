import { describe, expect, it } from 'vitest'
import type { CatalogStatus } from '@/api/types'
import { catalogRefetch, needsRecovery } from './catalog'

const status = (state: CatalogStatus['state']): CatalogStatus => ({
  state,
  problems: [],
  moved_to: null,
  checked_at: null,
  restored_from: null,
  last_backup_at: null,
  backup_dir: '/data/backups',
  backups: [],
})

describe('catalog status', () => {
  it('offers the backups only when the catalog is unreadable or damaged', () => {
    expect(needsRecovery(status('recovery'))).toBe(true)
    expect(needsRecovery(status('damaged'))).toBe(true)
    expect(needsRecovery(status('ok'))).toBe(false)
    expect(needsRecovery(status('checking'))).toBe(false)
    expect(needsRecovery(undefined)).toBe(false)
  })

  it('polls while the start-up check runs', () => {
    expect(catalogRefetch(status('checking'))).toBe(1500)
    expect(catalogRefetch(status('ok'))).toBe(false)
    expect(catalogRefetch(undefined)).toBe(false)
  })
})
