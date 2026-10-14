import { describe, expect, it } from 'vitest'
import { aspectOfPhoto, CANVAS_PRESETS, captionHeight, collageProblem, computeLayout, type Cell, type CollageLayoutKind, type Rect } from './collageLayout'

const r = (x: number, y: number, w: number, h: number): Rect => ({ x, y, w, h })
const plain = (rect: Rect): Cell => ({ photo: rect, caption: null })
const full = (c: Cell): Rect => (c.caption ? { ...c.photo, h: c.photo.h + c.caption.h } : c.photo)

describe('collage layout', () => {
  // the same golden rectangles are asserted in `golden_rectangles` in crates/ip-core/src/collage_tests.rs
  it('matches the server golden rectangles', () => {
    expect(computeLayout('grid', 1000, 1000, 10, 20, 0, [1.5, 1.5, 1.5, 1.5])).toEqual([plain(r(20, 20, 475, 475)), plain(r(505, 20, 475, 475)), plain(r(20, 505, 475, 475)), plain(r(505, 505, 475, 475))])
    expect(computeLayout('story', 1000, 500, 10, 0, 0, [1.5, 1.5, 1.5, 1.5])).toEqual([plain(r(0, 0, 660, 500)), plain(r(670, 0, 330, 160)), plain(r(670, 170, 330, 160)), plain(r(670, 340, 330, 160))])
    expect(computeLayout('contact', 400, 300, 10, 10, 20, [1.5, 1.5])).toEqual([
      { photo: r(10, 10, 185, 260), caption: r(10, 270, 185, 20) },
      { photo: r(205, 10, 185, 260), caption: r(205, 270, 185, 20) },
    ])
    expect(computeLayout('story', 500, 1000, 10, 0, 0, [1, 1])).toEqual([plain(r(0, 0, 500, 660)), plain(r(0, 670, 500, 330))])
    for (const kind of ['grid', 'story', 'contact'] as CollageLayoutKind[]) expect(computeLayout(kind, 300, 200, 10, 5, 0, [1.5])).toEqual([plain(r(5, 5, 290, 190))])
  })

  function checkTiling(cells: Cell[], w: number, h: number, gap: number, margin: number) {
    const rects = cells.map(full)
    rects.forEach((a, i) => {
      expect(a.w).toBeGreaterThan(0)
      expect(a.h).toBeGreaterThan(0)
      expect(a.x).toBeGreaterThanOrEqual(margin)
      expect(a.y).toBeGreaterThanOrEqual(margin)
      expect(a.x + a.w).toBeLessThanOrEqual(w - margin)
      expect(a.y + a.h).toBeLessThanOrEqual(h - margin)
      for (const b of rects.slice(i + 1)) {
        expect(a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y).toBe(true)
      }
      const right = rects.filter((b) => b.x >= a.x + a.w && b.y < a.y + a.h && a.y < b.y + b.h).map((b) => b.x - (a.x + a.w))
      if (right.length) expect(Math.min(...right)).toBe(gap)
      else expect(a.x + a.w).toBe(w - margin)
      const below = rects.filter((b) => b.y >= a.y + a.h && b.x < a.x + a.w && a.x < b.x + b.w).map((b) => b.y - (a.y + a.h))
      if (below.length) expect(Math.min(...below)).toBe(gap)
      else expect(a.y + a.h).toBe(h - margin)
      if (!rects.some((b) => b.x + b.w + gap <= a.x && b.y < a.y + a.h && a.y < b.y + b.h)) expect(a.x).toBe(margin)
      if (!rects.some((b) => b.y + b.h + gap <= a.y && b.x < a.x + a.w && a.x < b.x + b.w)) expect(a.y).toBe(margin)
    })
  }

  it('tiles the canvas without overlap and with exact gaps, or refuses', () => {
    const aspects = (n: number) => Array.from({ length: n }, (_, i) => [1.5, 0.667, 1, 1.778, 0.75][i % 5])
    for (const kind of ['grid', 'story', 'contact'] as CollageLayoutKind[]) {
      for (let n = 1; n <= 30; n++) {
        for (const [w, h, gap, margin] of [
          [240, 180, 4, 6],
          [150, 260, 0, 0],
          [300, 300, 7, 3],
          [400, 120, 2, 10],
        ]) {
          const caption = kind === 'contact' ? 9 : 0
          const cells = computeLayout(kind, w, h, gap, margin, caption, aspects(n))
          if (!cells) continue
          expect(cells).toHaveLength(n)
          checkTiling(cells, w, h, gap, margin)
          cells.forEach((c) => expect(c.caption !== null).toBe(caption > 0))
        }
      }
    }
  })

  it('is deterministic and the first photo is the large one of a story', () => {
    const aspects = [1.5, 0.667, 1, 1.778, 0.75, 1.5, 1]
    expect(computeLayout('grid', 1080, 1350, 12, 24, 0, aspects)).toEqual(computeLayout('grid', 1080, 1350, 12, 24, 0, aspects))
    const story = computeLayout('story', 1080, 1350, 12, 24, 0, aspects)!
    const area = (c: Cell) => c.photo.w * c.photo.h
    expect(story.slice(1).every((c) => area(c) < area(story[0]))).toBe(true)
    expect(computeLayout('grid', 1000, 1000, 0, 0, 0, [3, 3, 3])!.map((c) => c.photo.w)).toEqual([1000, 1000, 1000])
    expect(computeLayout('grid', 1000, 1000, 0, 0, 0, [0.33, 0.33, 0.33])!.map((c) => c.photo.h)).toEqual([1000, 1000, 1000])
  })

  it('refuses canvases that are too small', () => {
    expect(computeLayout('grid', 100, 100, 10, 50, 0, [1])).toBeNull()
    expect(computeLayout('grid', 100, 100, 40, 0, 0, Array(30).fill(1))).toBeNull()
    expect(computeLayout('contact', 100, 100, 0, 0, 100, [1, 1])).toBeNull()
    expect(computeLayout('story', 50, 50, 60, 0, 0, [1, 1, 1])).toBeNull()
  })

  it('has the presets of the PRD and checks the limits the server checks', () => {
    expect(CANVAS_PRESETS.map((p) => [p.width, p.height])).toEqual([
      [1080, 1350],
      [1080, 1080],
      [3840, 2160],
      [2480, 3508],
    ])
    expect(captionHeight(1080, 1350)).toBe(27)
    expect(captionHeight(100, 100)).toBe(14)
    const cells = computeLayout('grid', 1080, 1350, 16, 16, 0, [1.5, 1.5])
    expect(collageProblem(2, 1080, 1350, 16, 16, cells)).toBeNull()
    expect(collageProblem(1, 1080, 1350, 16, 16, cells)).toBe('count')
    expect(collageProblem(31, 1080, 1350, 16, 16, cells)).toBe('count')
    expect(collageProblem(2, 10, 1350, 16, 16, cells)).toBe('size')
    expect(collageProblem(2, 12000, 12000, 16, 16, cells)).toBe('size')
    expect(collageProblem(2, 1080, 1350, 2001, 16, cells)).toBe('spacing')
    expect(collageProblem(2, 1080, 1350, 16, 16, null)).toBe('tooSmall')
    expect(aspectOfPhoto({ width: 300, height: 200 })).toBe(1.5)
    expect(aspectOfPhoto({ width: null, height: null })).toBe(1.5)
  })
})
