import type { Photo } from '@/api/types'

export type CompareCount = 2 | 3 | 4
export const COMPARE_COUNTS: readonly CompareCount[] = [2, 3, 4]

/** Grid columns for `n` compared photos: 2-up and 3-up side by side, 4-up as 2x2. */
export function compareColumns(n: number): number {
  if (n <= 1) return 1
  return n === 3 ? 3 : 2
}

/** The selected photos in list order; compare uses them when two or more are selected. */
export function selectedInOrder(photos: readonly Photo[], selected: ReadonlySet<number>): Photo[] {
  return selected.size < 2 ? [] : photos.filter((p) => selected.has(p.id))
}

/**
 * Photos shown in compare mode. With two or more photos selected, the cells are the selection (in
 * list order), `count` at a time: the page holding the cursor photo, the first page when the cursor
 * is outside the selection. Otherwise: pinned A, active B, then the following candidates.
 */
export function compareSlots(
  photos: Photo[],
  aId: number | null,
  activeId: number | null,
  count: CompareCount,
  selected?: ReadonlySet<number>,
): Photo[] {
  const sel = selected ? selectedInOrder(photos, selected) : []
  if (sel.length >= 2) {
    const at = sel.findIndex((p) => p.id === activeId)
    const start = at < 0 ? 0 : Math.min(Math.floor(at / count) * count, Math.max(0, sel.length - count))
    return sel.slice(start, start + count)
  }
  const byId = (id: number | null) => (id === null ? undefined : photos.find((p) => p.id === id))
  const a = byId(aId)
  const b = byId(activeId)
  const out: Photo[] = []
  if (a) out.push(a)
  if (b && b !== a) out.push(b)
  if (b) {
    let i = photos.indexOf(b) + 1
    while (out.length < count && i < photos.length) {
      if (photos[i] !== a) out.push(photos[i])
      i++
    }
  }
  return out.slice(0, count)
}

/** Whether compare shows the selection (so there is no pinned A to swap). */
export function compareUsesSelection(photos: readonly Photo[], selected: ReadonlySet<number>): boolean {
  return selectedInOrder(photos, selected).length >= 2
}
