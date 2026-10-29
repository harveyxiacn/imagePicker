import type { GridItem, HeaderItem, PhotoItem } from './stacks'

/**
 * Packed grid layout for the grouped library. Scenes are laid out in order as segments on rows of a
 * fixed column count: a scene that fits in the space left on the current row continues there (after a
 * thin divider) instead of starting a new row, so a library of one-photo scenes still fills the width.
 * A scene that does not fit starts a new row and wraps normally. A collapsed scene takes one column
 * (a folded tile). Pure functions: the grid memoizes `packRows` on items + column count.
 */

export type PackedCell =
  | { kind: 'photo'; item: PhotoItem; /** first cell of a segment that starts mid-row (draw a divider before it) */ split: boolean }
  | { kind: 'folded'; header: HeaderItem; split: boolean }

/** A segment header drawn in the row's header strip, aligned to its first cells. */
export interface PackedHead {
  header: HeaderItem
  col: number
  /** Columns the segment occupies on this row. */
  span: number
}

export interface PackedRow {
  key: string
  /** Segments starting on this row; empty = the row continues a wrapped scene (no header strip). */
  heads: PackedHead[]
  /** Cells in column order; cell i sits in column i. */
  cells: PackedCell[]
}

interface Segment {
  header: HeaderItem | null
  photos: PhotoItem[]
}

/** Single linear pass over `items` (headers + photos as produced by `buildGridItems`). */
export function packRows(items: GridItem[], cols: number): PackedRow[] {
  const n = Math.max(1, Math.floor(cols))
  const rows: PackedRow[] = []
  let row: PackedRow | null = null

  const newRow = (): PackedRow => {
    row = { key: '', heads: [], cells: [] }
    rows.push(row)
    return row
  }
  const keyOf = (c: PackedCell) => (c.kind === 'photo' ? `p-${c.item.photo.id}` : `f-${c.header.key}`)

  const place = (seg: Segment) => {
    const size = seg.header?.collapsed ? 1 : seg.photos.length
    if (size === 0) return
    let cur: PackedRow = row ?? newRow()
    // Only start a new row when the segment does not fit in what is left of this one.
    if (cur.cells.length > 0 && size > n - cur.cells.length) cur = newRow()
    const split = cur.cells.length > 0
    if (seg.header) cur.heads.push({ header: seg.header, col: cur.cells.length, span: Math.min(size, n - cur.cells.length) })
    if (seg.header?.collapsed) {
      cur.cells.push({ kind: 'folded', header: seg.header, split })
      return
    }
    seg.photos.forEach((item, i) => {
      if (cur.cells.length >= n) cur = newRow()
      cur.cells.push({ kind: 'photo', item, split: split && i === 0 })
    })
  }

  let seg: Segment = { header: null, photos: [] }
  for (const it of items) {
    if (it.kind === 'header') {
      place(seg)
      seg = { header: it, photos: [] }
    } else seg.photos.push(it)
  }
  place(seg)
  for (const r of rows) r.key = keyOf(r.cells[0])
  return rows
}

/** Row and column of a photo, or null. */
export function locate(rows: PackedRow[], id: number | null): { row: number; col: number } | null {
  if (id === null) return null
  for (let r = 0; r < rows.length; r++) {
    const c = rows[r].cells.findIndex((cell) => cell.kind === 'photo' && cell.item.photo.id === id)
    if (c >= 0) return { row: r, col: c }
  }
  return null
}

/**
 * Id of the photo one visual row above/below `activeId`: the photo cell nearest to the same column
 * (folded scenes are skipped, as are rows holding none). Null at the edges or when not found.
 */
export function moveVertical(rows: PackedRow[], activeId: number | null, dir: 1 | -1): number | null {
  const at = locate(rows, activeId)
  if (!at) return null
  for (let r = at.row + dir; r >= 0 && r < rows.length; r += dir) {
    const cells = rows[r].cells
    let best = -1
    for (let c = 0; c < cells.length; c++) {
      // strict `<`: ties go to the left neighbour (reading order)
      if (cells[c].kind === 'photo' && (best < 0 || Math.abs(c - at.col) < Math.abs(best - at.col))) best = c
    }
    const hit = best >= 0 ? cells[best] : null
    if (hit?.kind === 'photo') return hit.item.photo.id
  }
  return null
}

/** Height of the header strip above a packed row that starts at least one scene. */
export const STRIP_H = 22

/** Left edge (px, inside the row) of column `col`, and the width of `span` columns. */
export const colX = (col: number, cell: number, gap: number) => col * (cell + gap)
export const spanW = (span: number, cell: number, gap: number) => span * cell + (span - 1) * gap
