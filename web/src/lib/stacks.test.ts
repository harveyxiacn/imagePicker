import { describe, expect, it } from 'vitest'
import type { Photo, Scene } from '@/api/types'
import { makePhoto } from './fixtures'
import {
  buildGridItems,
  buildRows,
  jumpGroup,
  makeSceneLookup,
  moveVertical,
  toggleStacks,
  visiblePhotos,
  type StackOptions,
} from './stacks'

// Scene 1: [1 single] [burst 10: 2,3,4 (best=3)]   Scene 2: [burst 11: 5,6 (best=5)] [7 single]
const burst = (id: number, burstId: number, size: number, rank: number, extra: Partial<Photo> = {}) =>
  makePhoto(id, { burst_id: burstId, burst_size: size, rank_in_burst: rank, analyzed: true, ...extra })

const photos: Photo[] = [
  makePhoto(1, { burst_id: 9, burst_size: 1, rank_in_burst: 0, taken_at: 1000 }),
  burst(2, 10, 3, 1, { taken_at: 2000 }),
  burst(3, 10, 3, 0, { taken_at: 2100 }),
  burst(4, 10, 3, 2, { taken_at: 2200 }),
  burst(5, 11, 2, 0, { taken_at: 90_000 }),
  burst(6, 11, 2, 1, { taken_at: 90_100 }),
  makePhoto(7, { burst_id: 12, burst_size: 1, rank_in_burst: 0, taken_at: 95_000 }),
]

const scenes: Scene[] = [
  {
    id: 1,
    start_at: 1000,
    end_at: 2200,
    bursts: [
      { id: 9, best_photo_id: 1, photo_ids: [1], size: 1, start_at: 1000 },
      { id: 10, best_photo_id: 3, photo_ids: [3, 2, 4], size: 3, start_at: 2000 },
    ],
  },
  {
    id: 2,
    start_at: 90_000,
    end_at: 95_000,
    bursts: [
      { id: 11, best_photo_id: 5, photo_ids: [5, 6], size: 2, start_at: 90_000 },
      { id: 12, best_photo_id: 7, photo_ids: [7], size: 1, start_at: 95_000 },
    ],
  },
]

const opts = (over: Partial<StackOptions> = {}): StackOptions => ({
  grouped: true,
  expandAll: false,
  expanded: new Set(),
  collapsedScenes: new Set(),
  scenes: undefined,
  withHeaders: false,
  ...over,
})

const ids = (items: ReturnType<typeof buildGridItems>) => visiblePhotos(items).map((p) => p.id)

describe('buildGridItems', () => {
  it('flat mode keeps every photo, no stacks', () => {
    const items = buildGridItems(photos, opts({ grouped: false }))
    expect(ids(items)).toEqual([1, 2, 3, 4, 5, 6, 7])
    expect(items.every((i) => i.kind === 'photo' && i.stack === null)).toBe(true)
  })

  it('collapses bursts to their best shot with a count; size-1 bursts are not stacks', () => {
    const items = buildGridItems(photos, opts())
    expect(ids(items)).toEqual([1, 3, 5, 7])
    const stack = items.find((i) => i.kind === 'photo' && i.photo.id === 3)
    expect(stack).toMatchObject({ stack: { burstId: 10, count: 3, expanded: false, isCover: true } })
    expect(items.find((i) => i.kind === 'photo' && i.photo.id === 1)).toMatchObject({ stack: null })
  })

  it('expands one stack, members ordered by rank (best first)', () => {
    const items = buildGridItems(photos, opts({ expanded: new Set([10]) }))
    expect(ids(items)).toEqual([1, 3, 2, 4, 5, 7])
    const members = items.filter((i) => i.kind === 'photo' && i.stack?.burstId === 10)
    expect(members.map((m) => m.kind === 'photo' && m.stack?.isCover)).toEqual([true, false, false])
  })

  it('expandAll expands every stack', () => {
    expect(ids(buildGridItems(photos, opts({ expandAll: true })))).toEqual([1, 3, 2, 4, 5, 6, 7])
  })

  it('counts only members present after filtering and falls back to the best present member', () => {
    // best shot (3) filtered out: stack of 2 and 4 shows 2 (rank 1)
    const filtered = photos.filter((p) => p.id !== 3)
    const items = buildGridItems(filtered, opts())
    expect(ids(items)).toEqual([1, 2, 5, 7])
    expect(items.find((i) => i.kind === 'photo' && i.photo.id === 2)).toMatchObject({ stack: { count: 2 } })
    // a lone survivor is a plain photo, not a stack
    const lone = photos.filter((p) => p.id !== 3 && p.id !== 4)
    expect(buildGridItems(lone, opts()).find((i) => i.kind === 'photo' && i.photo.id === 2)).toMatchObject({ stack: null })
  })

  it('places the stack where its first member appears (time order), not at the best shot', () => {
    expect(ids(buildGridItems(photos, opts()))).toEqual([1, 3, 5, 7])
    // sorted by AI score: best shots first
    const byAi = [photos[2], photos[4], photos[0], photos[1], photos[3], photos[5], photos[6]]
    expect(ids(buildGridItems(byAi, opts()))).toEqual([3, 5, 1, 7])
  })

  it('inserts scene headers with counts and supports collapsing a scene', () => {
    const items = buildGridItems(photos, opts({ scenes, withHeaders: true }))
    const headers = items.filter((i) => i.kind === 'header')
    expect(headers).toHaveLength(2)
    expect(headers[0]).toMatchObject({ sceneId: 1, index: 1, photoCount: 4, groupCount: 2, collapsed: false })
    expect(headers[1]).toMatchObject({ sceneId: 2, index: 2, photoCount: 3, groupCount: 2 })
    expect(items[0].kind).toBe('header')
    expect(items.map((i) => (i.kind === 'header' ? `H${i.sceneId}` : i.photo.id))).toEqual(['H1', 1, 3, 'H2', 5, 7])

    const collapsed = buildGridItems(photos, opts({ scenes, withHeaders: true, collapsedScenes: new Set([1]) }))
    expect(collapsed.map((i) => (i.kind === 'header' ? `H${i.sceneId}` : i.photo.id))).toEqual(['H1', 'H2', 5, 7])
    expect(collapsed[0]).toMatchObject({ collapsed: true })
  })

  it('omits headers when disabled (non-time sort) or scenes are unknown', () => {
    expect(buildGridItems(photos, opts({ scenes, withHeaders: false })).some((i) => i.kind === 'header')).toBe(false)
    expect(buildGridItems(photos, opts({ scenes: [], withHeaders: true })).some((i) => i.kind === 'header')).toBe(false)
  })

  it('gives each run of a non-contiguous scene its own unique header key', () => {
    // Photo 8 belongs to scene 1 (burst 9) but sorts after a scene-2 photo, e.g. no EXIF time.
    const interleaved = [photos[0], photos[6], makePhoto(8, { burst_id: 9, burst_size: 1, rank_in_burst: 0 })]
    const headers = buildGridItems(interleaved, opts({ scenes, withHeaders: true })).filter((i) => i.kind === 'header')
    expect(headers.map((h) => h.sceneId)).toEqual([1, 2, 1])
    const keys = headers.map((h) => h.key)
    expect(new Set(keys).size).toBe(keys.length)
  })
})

describe('makeSceneLookup', () => {
  it('uses burst membership, then capture time', () => {
    const of = makeSceneLookup(scenes)
    expect(of(photos[2])).toBe(1)
    expect(of(photos[4])).toBe(2)
    // not in any burst yet (e.g. analysed later): fall back to time
    expect(of(makePhoto(99, { taken_at: 2150 }))).toBe(1)
    expect(of(makePhoto(98, { taken_at: 90_050 }))).toBe(2)
    expect(of(makePhoto(97, { taken_at: 50_000 }))).toBeNull()
    expect(of(makePhoto(96, { taken_at: null }))).toBeNull()
  })
})

describe('rows & vertical navigation', () => {
  const items = buildGridItems(photos, opts({ scenes, withHeaders: true }))
  const rows = buildRows(items, 2)

  it('starts a new row after every header', () => {
    expect(rows.map((r) => (r.kind === 'header' ? 'H' : r.items.map((i) => i.photo.id).join(',')))).toEqual(['H', '1,3', 'H', '5,7'])
  })

  it('moves up/down keeping the column and skipping headers', () => {
    expect(moveVertical(rows, 1, 1)).toBe(5)
    expect(moveVertical(rows, 3, 1)).toBe(7)
    expect(moveVertical(rows, 7, -1)).toBe(3)
    expect(moveVertical(rows, 1, -1)).toBeNull()
    expect(moveVertical(rows, 7, 1)).toBeNull()
    expect(moveVertical(rows, 999, 1)).toBeNull()
  })

  it('clamps the column on shorter rows', () => {
    const r = buildRows(buildGridItems(photos, opts({ grouped: false })), 3)
    // rows: [1,2,3] [4,5,6] [7]
    expect(moveVertical(r, 6, 1)).toBe(7)
    expect(moveVertical(r, 7, -1)).toBe(4)
  })
})

describe('jumpGroup', () => {
  const flat = photos.filter((p) => [1, 3, 5, 7].includes(p.id))
  it('jumps between stacks, skipping singles', () => {
    expect(jumpGroup(flat, 1, 1)).toBe(3)
    expect(jumpGroup(flat, 3, 1)).toBe(5)
    expect(jumpGroup(flat, 5, 1)).toBeNull()
    expect(jumpGroup(flat, 7, -1)).toBe(5)
    expect(jumpGroup(flat, 5, -1)).toBe(3)
    expect(jumpGroup(flat, 3, -1)).toBeNull()
  })

  it('lands on the first visible photo of a group and leaves the current one when expanded', () => {
    const expanded = [photos[0], photos[2], photos[1], photos[3], photos[4], photos[6]] // 1, 3, 2, 4, 5, 7
    expect(jumpGroup(expanded, 3, 1)).toBe(5)
    expect(jumpGroup(expanded, 4, 1)).toBe(5)
    expect(jumpGroup(expanded, 5, -1)).toBe(3)
    expect(jumpGroup(expanded, 4, -1)).toBeNull()
  })
})

describe('toggleStacks (S key)', () => {
  const none = { expandAll: false, expanded: new Set<number>() }
  it('toggles the stack under the cursor', () => {
    const on = toggleStacks(none, photos[2])
    expect([...on.expanded]).toEqual([10])
    expect(on.expandAll).toBe(false)
    expect(toggleStacks(on, photos[2]).expanded.size).toBe(0)
  })

  it('toggles all when the cursor is not on a stack, or with the all flag', () => {
    expect(toggleStacks(none, photos[0]).expandAll).toBe(true)
    expect(toggleStacks(none, photos[2], true).expandAll).toBe(true)
    expect(toggleStacks({ expandAll: true, expanded: new Set() }, photos[0])).toMatchObject({ expandAll: false })
  })

  it('collapses everything when expand-all is on and the cursor is on a stack', () => {
    const r = toggleStacks({ expandAll: true, expanded: new Set() }, photos[2])
    expect(r.expandAll).toBe(false)
    expect(r.expanded.size).toBe(0)
  })

  it('"all" collapses individually expanded stacks first', () => {
    expect(toggleStacks({ expandAll: false, expanded: new Set([10]) }, photos[0])).toMatchObject({ expandAll: false })
  })
})
