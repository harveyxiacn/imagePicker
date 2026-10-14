import { describe, expect, it } from 'vitest'
import { canvasOf, captionOf, collageBody, DEFAULT_COLLAGE, isColor, previewCells, problemOf } from './collageOptions'

describe('collage options', () => {
  it('maps presets and custom sizes to canvas pixels', () => {
    expect(canvasOf(DEFAULT_COLLAGE)).toEqual({ width: 1080, height: 1350 })
    expect(canvasOf({ ...DEFAULT_COLLAGE, preset: 'a4' })).toEqual({ width: 2480, height: 3508 })
    expect(canvasOf({ ...DEFAULT_COLLAGE, preset: 'custom', customWidth: 900, customHeight: 600 })).toEqual({ width: 900, height: 600 })
  })

  it('previews the layout for the photos aspect ratios', () => {
    const cells = previewCells({ ...DEFAULT_COLLAGE, preset: '1:1', gap: 10, margin: 20 }, [1.5, 1.5, 1.5, 1.5])!
    expect(cells.map((c) => c.photo)).toEqual([
      { x: 20, y: 20, w: 515, h: 515 },
      { x: 545, y: 20, w: 515, h: 515 },
      { x: 20, y: 545, w: 515, h: 515 },
      { x: 545, y: 545, w: 515, h: 515 },
    ])
    // caption bands only on contact sheets with a caption on
    expect(captionOf(DEFAULT_COLLAGE)).toBe(0)
    expect(captionOf({ ...DEFAULT_COLLAGE, layout: 'contact' })).toBe(27)
    expect(captionOf({ ...DEFAULT_COLLAGE, layout: 'contact', captionName: false, captionRating: false })).toBe(0)
    expect(previewCells({ ...DEFAULT_COLLAGE, layout: 'contact' }, [1.5, 1.5])!.every((c) => c.caption !== null)).toBe(true)
  })

  it('reports why a collage cannot be exported', () => {
    const a = (n: number) => Array(n).fill(1.5)
    expect(problemOf(DEFAULT_COLLAGE, a(6))).toBeNull()
    expect(problemOf(DEFAULT_COLLAGE, a(1))).toBe('count')
    expect(problemOf(DEFAULT_COLLAGE, a(31))).toBe('count')
    expect(problemOf({ ...DEFAULT_COLLAGE, preset: 'custom', customWidth: 20 }, a(4))).toBe('size')
    expect(problemOf({ ...DEFAULT_COLLAGE, margin: 700 }, a(4))).toBe('tooSmall')
  })

  it('builds the request body', () => {
    expect(collageBody({ ...DEFAULT_COLLAGE, layout: 'story', background: '#101010' }, [3, 1, 2], 'D:\\out', { format: 'original', quality: 90, template: ' ', watermark: null })).toEqual({
      ids: [3, 1, 2],
      dest: 'D:\\out',
      layout: 'story',
      width: 1080,
      height: 1350,
      gap: 16,
      margin: 16,
      background: '#101010',
      fit: 'cover',
      caption_name: true,
      caption_rating: true,
      format: 'jpeg',
      quality: 90,
      name_template: 'collage',
    })
    const wm = { kind: 'text' as const, text: '© {year}', position: 'bottom-right' as const, margin_pct: 2, opacity: 0.6 }
    const b = collageBody(DEFAULT_COLLAGE, [1, 2], 'x', { format: 'png', quality: 80, template: 'sheet_{name}', watermark: wm })
    expect(b).toMatchObject({ format: 'png', name_template: 'sheet_{name}', watermark: wm })
  })

  it('accepts the colours the server accepts', () => {
    expect(['#fff', '#A0b1C2', '#12345'].map(isColor)).toEqual([true, true, false])
  })
})
