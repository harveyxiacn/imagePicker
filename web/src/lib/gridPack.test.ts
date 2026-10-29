import { describe, expect, it } from 'vitest'
import { makePhoto } from './fixtures'
import { locate, moveVertical, packRows, type PackedRow } from './gridPack'
import type { GridItem, HeaderItem, PhotoItem } from './stacks'

let nextId = 1
const header = (sceneId: number, collapsed = false, photoCount = 1): HeaderItem => ({
  kind: 'header',
  key: `scene-${sceneId}`,
  sceneId,
  index: sceneId,
  startAt: 0,
  endAt: 0,
  offsetMin: null,
  photoCount,
  groupCount: 1,
  collapsed,
})
const photo = (): PhotoItem => ({ kind: 'photo', photo: makePhoto(nextId++), stack: null })

/** Scenes given by their photo counts; a negative count = collapsed scene (no photos emitted). */
function scenes(...sizes: number[]): GridItem[] {
  nextId = 1
  const out: GridItem[] = []
  sizes.forEach((n, i) => {
    out.push(header(i + 1, n < 0, Math.abs(n)))
    if (n > 0) for (let k = 0; k < n; k++) out.push(photo())
  })
  return out
}

/** Compact picture of a layout: one string per row, `|` = segment boundary, `F` = folded scene. */
const draw = (rows: PackedRow[]) =>
  rows.map((r) =>
    r.cells.map((c) => `${c.split ? '|' : ''}${c.kind === 'photo' ? c.item.photo.id : `F${c.header.sceneId}`}`).join(' '),
  )
const photoIds = (rows: PackedRow[]) =>
  rows.flatMap((r) => r.cells.flatMap((c) => (c.kind === 'photo' ? [c.item.photo.id] : [])))
const inputIds = (items: GridItem[]) => items.flatMap((i) => (i.kind === 'photo' ? [i.photo.id] : []))

describe('packRows', () => {
  it('packs small scenes onto one row and wraps when the next does not fit', () => {
    // scenes of 1, 1, 2, 3 photos on 5 columns: 1+1+2 fit, the 3-photo scene starts a new row
    const rows = packRows(scenes(1, 1, 2, 3), 5)
    expect(draw(rows)).toEqual(['1 |2 |3 4', '5 6 7'])
    expect(rows[0].heads.map((h) => [h.header.sceneId, h.col, h.span])).toEqual([
      [1, 0, 1],
      [2, 1, 1],
      [3, 2, 2],
    ])
  })

  it('wraps a scene larger than a row normally, and later scenes may fill its last row', () => {
    const rows = packRows(scenes(1, 7, 1), 4)
    // the 7-photo scene does not fit beside scene 1 -> new row; scene 3 fits after its tail
    expect(draw(rows)).toEqual(['1', '2 3 4 5', '6 7 8 |9'])
    expect(rows[1].heads).toEqual([{ header: expect.objectContaining({ sceneId: 2 }), col: 0, span: 4 }])
    expect(rows[2].heads.map((h) => [h.header.sceneId, h.col])).toEqual([[3, 3]])
  })

  it('never splits a scene that fits in a row', () => {
    const sizes = [1, 3, 2, 4, 1, 1, 5, 2, 3, 1, 6, 1]
    for (const cols of [1, 2, 3, 4, 5, 6, 8]) {
      const rows = packRows(scenes(...sizes), cols)
      const rowOf = new Map<number, number>()
      rows.forEach((r, ri) => r.cells.forEach((c) => c.kind === 'photo' && rowOf.set(c.item.photo.id, ri)))
      let id = 1
      for (const n of sizes) {
        const used = new Set(Array.from({ length: n }, (_, k) => rowOf.get(id + k)))
        // a scene spans exactly ceil(n / cols) rows: one row if it fits
        expect(used.size).toBe(Math.ceil(n / cols))
        id += n
      }
      for (const r of rows) expect(r.cells.length).toBeLessThanOrEqual(cols)
    }
  })

  it('keeps every photo exactly once, in order, for any column count', () => {
    const items = scenes(2, 1, 1, 9, 1, 3, 4, 1, 1, 1)
    for (let cols = 1; cols <= 12; cols++) {
      const ids = photoIds(packRows(items, cols))
      expect(ids).toEqual(inputIds(items))
      expect(new Set(ids).size).toBe(ids.length)
    }
  })

  it('gives a collapsed scene one folded column that packs like a one-photo scene', () => {
    const rows = packRows(scenes(1, -5, 2, -3), 4)
    expect(draw(rows)).toEqual(['1 |F2 |2 3', 'F4'])
    expect(rows[0].heads[1]).toMatchObject({ col: 1, span: 1, header: { collapsed: true, photoCount: 5 } })
  })

  it('lays a list without headers (flat view) out in plain rows', () => {
    nextId = 1
    const flat = Array.from({ length: 7 }, photo)
    const rows = packRows(flat, 3)
    expect(draw(rows)).toEqual(['1 2 3', '4 5 6', '7'])
    expect(rows.every((r) => r.heads.length === 0)).toBe(true)
  })

  it('handles the degenerate cases', () => {
    expect(packRows([], 5)).toEqual([])
    expect(draw(packRows(scenes(2, 1), 0))).toEqual(['1', '2', '3']) // clamped to one column
    expect(packRows([header(1)], 5)).toEqual([]) // a scene with nothing to show takes no space
  })

  it('reads each input item once (a single linear pass)', () => {
    const items = scenes(3, 1, 1, 2, 8, 1)
    let reads = 0
    const counted = new Proxy(items, {
      get(target, prop, recv) {
        if (typeof prop === 'string' && /^\d+$/.test(prop)) reads++
        return Reflect.get(target, prop, recv)
      },
    })
    packRows(counted, 4)
    expect(reads).toBe(items.length)
  })

  it('keys rows by their first cell (stable across re-layouts)', () => {
    const rows = packRows(scenes(1, -2, 1), 2)
    expect(rows.map((r) => r.key)).toEqual(['p-1', 'p-2'])
  })
})

describe('vertical navigation on packed rows', () => {
  // scene 9 is collapsed; see the first test for the layout
  const items = scenes(1, 1, 2, 5)
  items.push(header(9, true, 3))
  nextId = 10
  items.push(header(10), photo())
  const rows = packRows(items, 4)

  it('lays out the fixture as expected', () => {
    expect(draw(rows)).toEqual(['1 |2 |3 4', '5 6 7 8', '9 |F9 |10'])
  })

  it('moves to the same column of the next / previous row', () => {
    expect(moveVertical(rows, 3, 1)).toBe(7)
    expect(moveVertical(rows, 8, -1)).toBe(4)
    expect(moveVertical(rows, 6, 1)).toBe(9) // column 1 is folded: nearest photo, left wins the tie
    expect(moveVertical(rows, 7, 1)).toBe(10)
    expect(moveVertical(rows, 8, 1)).toBe(10) // short row: nearest existing column
  })

  it('stops at the edges and on unknown ids', () => {
    expect(moveVertical(rows, 2, -1)).toBeNull()
    expect(moveVertical(rows, 10, 1)).toBeNull()
    expect(moveVertical(rows, 999, 1)).toBeNull()
    expect(moveVertical(rows, null, 1)).toBeNull()
  })

  it('skips rows that hold only folded scenes', () => {
    const r = packRows([...scenes(2), header(5, true), header(6, true)], 1)
    // cols 1: '1', '2', 'F5', 'F6'
    expect(moveVertical(r, 2, 1)).toBeNull()
    expect(locate(r, 2)).toEqual({ row: 1, col: 0 })
  })
})
