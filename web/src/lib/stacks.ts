import type { Photo, Scene } from '@/api/types'

/** A burst only counts as a stack when at least two of its photos are present. */
export interface StackInfo {
  burstId: number
  /** Number of members present in the current list. */
  count: number
  expanded: boolean
  /** Best shot of the stack (the one shown while collapsed). */
  isCover: boolean
}

export interface HeaderItem {
  kind: 'header'
  key: string
  sceneId: number
  /** 1-based position among scenes */
  index: number
  startAt: number
  endAt: number
  /** UTC offset of the first photo shown under the header (for local-time display) */
  offsetMin: number | null
  photoCount: number
  groupCount: number
  collapsed: boolean
}

export interface PhotoItem {
  kind: 'photo'
  photo: Photo
  stack: StackInfo | null
}

export type GridItem = HeaderItem | PhotoItem

export interface StackOptions {
  /** false = flat list: no stacks, no scene headers. */
  grouped: boolean
  expandAll: boolean
  expanded: ReadonlySet<number>
  collapsedScenes: ReadonlySet<number>
  scenes: Scene[] | undefined
  /** Scene headers only make sense in time order. */
  withHeaders: boolean
}

export const isStackMember = (p: Photo): boolean => p.burst_id !== null && (p.burst_size ?? 0) > 1

const rankOf = (p: Photo) => p.rank_in_burst ?? Number.MAX_SAFE_INTEGER

/** Lookup photo -> scene id (burst membership first, then capture time). */
export function makeSceneLookup(scenes: Scene[] | undefined): (p: Photo) => number | null {
  if (!scenes?.length) return () => null
  const byBurst = new Map<number, number>()
  for (const s of scenes) for (const b of s.bursts) byBurst.set(b.id, s.id)
  const sorted = [...scenes].sort((a, b) => a.start_at - b.start_at)
  return (p) => {
    if (p.burst_id !== null) {
      const s = byBurst.get(p.burst_id)
      if (s !== undefined) return s
    }
    if (p.taken_at === null) return null
    const t = p.taken_at
    let lo = 0
    let hi = sorted.length - 1
    let found = -1
    while (lo <= hi) {
      const mid = (lo + hi) >> 1
      if (sorted[mid].start_at <= t) {
        found = mid
        lo = mid + 1
      } else hi = mid - 1
    }
    return found >= 0 && t <= sorted[found].end_at ? sorted[found].id : null
  }
}

export function buildGridItems(photos: Photo[], opts: StackOptions): GridItem[] {
  if (!opts.grouped) return photos.map((photo) => ({ kind: 'photo', photo, stack: null }))

  // Present members per burst.
  const members = new Map<number, Photo[]>()
  for (const p of photos) {
    if (!isStackMember(p)) continue
    const list = members.get(p.burst_id!)
    if (list) list.push(p)
    else members.set(p.burst_id!, [p])
  }
  for (const [id, list] of members) {
    if (list.length < 2) members.delete(id)
    else list.sort((a, b) => rankOf(a) - rankOf(b) || a.id - b.id)
  }

  const flat: PhotoItem[] = []
  const emitted = new Set<number>()
  for (const p of photos) {
    const list = p.burst_id !== null ? members.get(p.burst_id) : undefined
    if (!list) {
      flat.push({ kind: 'photo', photo: p, stack: null })
      continue
    }
    const id = p.burst_id!
    if (emitted.has(id)) continue
    emitted.add(id)
    const expanded = opts.expandAll || opts.expanded.has(id)
    const shown = expanded ? list : [list[0]]
    shown.forEach((m, i) =>
      flat.push({ kind: 'photo', photo: m, stack: { burstId: id, count: list.length, expanded, isCover: i === 0 } }),
    )
  }

  if (!opts.withHeaders || !opts.scenes?.length) return flat

  const sceneOf = makeSceneLookup(opts.scenes)
  const sceneById = new Map(opts.scenes.map((s, i) => [s.id, { s, index: i + 1 }]))
  // Photos per scene (counting every present photo, including hidden stack members).
  const counts = new Map<number, number>()
  for (const p of photos) {
    const sid = sceneOf(p)
    if (sid !== null) counts.set(sid, (counts.get(sid) ?? 0) + 1)
  }

  const out: GridItem[] = []
  // A scene can reappear non-contiguously (e.g. photos without EXIF time sort elsewhere);
  // each run gets its own header, so keys must be unique per occurrence.
  const seen = new Map<number, number>()
  let current: number | null | undefined
  let hideCurrent = false
  for (const item of flat) {
    const sid = sceneOf(item.photo)
    if (sid !== current) {
      current = sid
      hideCurrent = false
      const info = sid !== null ? sceneById.get(sid) : undefined
      if (info && sid !== null) {
        const collapsed = opts.collapsedScenes.has(sid)
        hideCurrent = collapsed
        const occurrence = seen.get(sid) ?? 0
        seen.set(sid, occurrence + 1)
        out.push({
          kind: 'header',
          key: occurrence === 0 ? `scene-${sid}` : `scene-${sid}-${occurrence}`,
          sceneId: sid,
          index: info.index,
          startAt: info.s.start_at,
          endAt: info.s.end_at,
          offsetMin: item.photo.taken_at_offset_min,
          photoCount: counts.get(sid) ?? 0,
          groupCount: info.s.bursts.length,
          collapsed,
        })
      }
    }
    if (!hideCurrent) out.push(item)
  }
  return out
}

export const visiblePhotos = (items: GridItem[]): Photo[] =>
  items.filter((i): i is PhotoItem => i.kind === 'photo').map((i) => i.photo)

export type GridRow = { kind: 'header'; item: HeaderItem } | { kind: 'photos'; items: PhotoItem[] }

/** Lay items out in rows of `cols`; every header starts a new row and breaks the photo run. */
export function buildRows(items: GridItem[], cols: number): GridRow[] {
  const rows: GridRow[] = []
  let run: PhotoItem[] | null = null
  for (const it of items) {
    if (it.kind === 'header') {
      run = null
      rows.push({ kind: 'header', item: it })
      continue
    }
    if (!run || run.length >= cols) {
      run = []
      rows.push({ kind: 'photos', items: run })
    }
    run.push(it)
  }
  return rows
}

/** Id of the photo one visual row above/below `activeId` (same column, clamped); skips header rows. */
export function moveVertical(rows: GridRow[], activeId: number | null, dir: 1 | -1): number | null {
  let r = -1
  let c = 0
  rows.forEach((row, ri) => {
    if (row.kind !== 'photos') return
    const ci = row.items.findIndex((i) => i.photo.id === activeId)
    if (ci >= 0) {
      r = ri
      c = ci
    }
  })
  if (r < 0) return null
  for (let i = r + dir; i >= 0 && i < rows.length; i += dir) {
    const row = rows[i]
    if (row.kind === 'photos') return row.items[Math.min(c, row.items.length - 1)].photo.id
  }
  return null
}

/** First photo of the previous/next stack relative to the active photo (`,` / `.`). */
export function jumpGroup(photos: Photo[], activeId: number | null, dir: 1 | -1): number | null {
  const idx = photos.findIndex((p) => p.id === activeId)
  const cur = idx >= 0 && isStackMember(photos[idx]) ? photos[idx].burst_id : null
  const start = idx < 0 ? (dir > 0 ? -1 : photos.length) : idx
  for (let i = start + dir; i >= 0 && i < photos.length; i += dir) {
    const p = photos[i]
    if (!isStackMember(p) || p.burst_id === cur) continue
    let first = i
    while (first - 1 >= 0 && photos[first - 1].burst_id === p.burst_id) first--
    return photos[first].id
  }
  return null
}

export function toggleInSet(set: ReadonlySet<number>, id: number): Set<number> {
  const next = new Set(set)
  if (next.has(id)) next.delete(id)
  else next.add(id)
  return next
}

export interface StackExpansion {
  expandAll: boolean
  expanded: ReadonlySet<number>
}

/**
 * `S` semantics: with the cursor on a stack photo toggle that stack; otherwise (or with `all`)
 * toggle every stack. While "expand all" is on, `S` on a stack collapses everything.
 */
export function toggleStacks(state: StackExpansion, active: Photo | undefined, all = false): StackExpansion {
  if (!all && active && isStackMember(active) && !state.expandAll) {
    return { expandAll: false, expanded: toggleInSet(state.expanded, active.burst_id!) }
  }
  if (state.expandAll || state.expanded.size > 0) return { expandAll: false, expanded: new Set() }
  return { expandAll: true, expanded: new Set() }
}
