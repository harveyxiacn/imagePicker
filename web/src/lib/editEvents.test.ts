import { QueryClient } from '@tanstack/react-query'
import { describe, expect, it } from 'vitest'
import type { ServerEvent } from '@/api/types'
import { applyEvents, qk, type PhotosData } from './cache'
import { makePhoto } from './fixtures'
import { buildPhotosQuery, DEFAULT_FILTER } from './filter'

describe('edits.updated event', () => {
  it('patches has_edits + thumb_version (latest per id wins) and invalidates cached stacks', () => {
    const qc = new QueryClient()
    const key = qk.photos(1, buildPhotosQuery(1, DEFAULT_FILTER))
    qc.setQueryData<PhotosData>(key, { photos: [makePhoto(1), makePhoto(2), makePhoto(3)], total: 3 })
    qc.setQueryData(qk.edits(2), { photo_id: 2, stack: { version: 1, ops: [] }, updated_at: null })
    qc.setQueryData(qk.edits(3), { photo_id: 3, stack: { version: 1, ops: [] }, updated_at: null })
    const events: ServerEvent[] = [
      { type: 'edits.updated', items: [{ id: 2, has_edits: true, thumb_version: 'e1' }] },
      {
        type: 'edits.updated',
        items: [
          { id: 2, has_edits: true, thumb_version: 'e2' },
          { id: 1, has_edits: false, thumb_version: 'v9' },
        ],
      },
    ]
    const r = applyEvents(qc, events)
    expect(r.patchedIds).toBe(2)
    const list = qc.getQueryData<PhotosData>(key)!.photos
    expect(list[1]).toMatchObject({ id: 2, has_edits: true, thumb_version: 'e2' })
    expect(list[0]).toMatchObject({ id: 1, has_edits: false, thumb_version: 'v9' })
    expect(list[2].has_edits).toBe(false)
    expect(qc.getQueryState(qk.edits(2))?.isInvalidated).toBe(true)
    expect(qc.getQueryState(qk.edits(3))?.isInvalidated).toBe(false)
  })
})
