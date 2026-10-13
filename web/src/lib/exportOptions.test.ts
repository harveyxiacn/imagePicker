import { describe, expect, it } from 'vitest'
import type { ExportPreset } from '@/api/types'
import { applyPreset, aspectOf, DEFAULT_FORM, DEFAULT_WATERMARK, exampleName, exportBody, extensionOf, loadWatermark, parseWatermark, saveWatermark, usesQuality, watermarkBody, watermarkIncomplete } from './exportOptions'

const xhs: ExportPreset = { id: 'xiaohongshu', long_edge: 1920, aspect: [3, 4], quality: 90, format: 'jpeg', strip_gps: true }
const original: ExportPreset = { id: 'original', long_edge: null, aspect: null, quality: 95, format: 'original', strip_gps: false }

describe('export options', () => {
  it('applies a server preset: size, crop, quality, format and GPS', () => {
    const f = applyPreset({ ...DEFAULT_FORM, stripMetadata: true }, xhs)
    expect(f).toMatchObject({ edge: 'custom', custom: 1920, aspect: '3:4', quality: 90, format: 'jpeg', stripGps: true, stripMetadata: false })
    const o = applyPreset(f, original)
    expect(o).toMatchObject({ edge: 'original', aspect: 'none', quality: 95, format: 'original', stripGps: false })
    expect(applyPreset(DEFAULT_FORM, { ...xhs, long_edge: 2048, aspect: null }).edge).toBe('2048')
  })

  it('builds the request body', () => {
    const f = applyPreset(DEFAULT_FORM, xhs)
    expect(exportBody(f, { ids: [1, 2] }, 'D:\\out', '', 0)).toEqual({
      ids: [1, 2],
      dest: 'D:\\out',
      long_edge: 1920,
      quality: 90,
      name_template: '{name}',
      format: 'jpeg',
      aspect: [3, 4],
      strip_gps: true,
      strip_metadata: false,
    })
    const bare = exportBody({ ...DEFAULT_FORM, edge: 'original', format: 'webp', stripMetadata: true }, { folders: { Ann: [3] } }, 'x', '{event}_{seq}', 2)
    expect(bare).toMatchObject({ folders: { Ann: [3] }, long_edge: null, format: 'webp', aspect: null, strip_gps: true, strip_metadata: true, upscale: 2, name_template: '{event}_{seq}' })
  })

  it('parses aspect choices', () => {
    expect(aspectOf('4:5')).toEqual([4, 5])
    expect(aspectOf('none')).toBeNull()
    expect(aspectOf('0:5')).toBeNull()
    expect(aspectOf('3:4:5')).toBeNull()
  })

  it('names the example with every token and the format extension', () => {
    expect(exampleName('{date}_{event}_{scene}_{camera}_{rating}_{seq}', 'tiff')).toBe('20240229_Kyoto_portrait_Canon EOS R5_4_0001.tif')
    expect(exampleName('', 'original')).toBe('IMG_0001.jpg')
    expect(exampleName('{name}_{unknown}', 'webp')).toBe('IMG_0001_{unknown}.webp')
    expect(extensionOf('png')).toBe('png')
    expect(usesQuality('png')).toBe(false)
    expect(usesQuality('original')).toBe(true)
  })

  describe('watermark', () => {
    const on = { ...DEFAULT_WATERMARK, enabled: true }

    it('is left out of the body unless enabled and complete', () => {
      expect(exportBody(DEFAULT_FORM, { ids: [1] }, 'x', '', 0)).not.toHaveProperty('watermark')
      expect(watermarkBody({ ...on, enabled: false })).toBeNull()
      expect(watermarkBody({ ...on, text: '  ' })).toBeNull()
      expect(watermarkIncomplete({ ...on, text: '' })).toBe(true)
      expect(watermarkIncomplete({ ...on, kind: 'image' })).toBe(true)
      expect(watermarkIncomplete(DEFAULT_WATERMARK)).toBe(false)
      expect(watermarkIncomplete(on)).toBe(false)
    })

    it('serialises a text mark: percent opacity to 0..1, only text fields', () => {
      const body = exportBody({ ...DEFAULT_FORM, watermark: { ...on, text: '© {camera}', position: 'top-left', opacity: 35, fontSizePct: 4.5, color: '#ff8800', shadow: false, imagePath: '/ignored.png' } }, { ids: [1] }, 'x', '', 0)
      expect(body.watermark).toEqual({ kind: 'text', text: '© {camera}', font_size_pct: 4.5, color: '#ff8800', shadow: false, position: 'top-left', margin_pct: 2, opacity: 0.35 })
      expect(watermarkBody({ ...on, fontPath: ' /fonts/Noto.otf ' })).toMatchObject({ font_path: '/fonts/Noto.otf' })
    })

    it('serialises an image mark', () => {
      expect(watermarkBody({ ...on, kind: 'image', imagePath: '/logo.png', sizePct: 15, marginPct: 4, opacity: 100, position: 'center', text: 'ignored' })).toEqual({
        kind: 'image',
        image_path: '/logo.png',
        size_pct: 15,
        position: 'center',
        margin_pct: 4,
        opacity: 1,
      })
    })

    it('is kept across a server preset', () => {
      const f = applyPreset({ ...DEFAULT_FORM, watermark: on }, xhs)
      expect(f.watermark).toEqual(on)
    })

    it('remembers the last used settings and survives garbage', () => {
      const w = { ...on, text: 'mine', position: 'bottom-left' as const, opacity: 80 }
      saveWatermark(w)
      expect(loadWatermark()).toEqual(w)
      expect(parseWatermark(null)).toEqual(DEFAULT_WATERMARK)
      expect(parseWatermark('not json')).toEqual(DEFAULT_WATERMARK)
      expect(parseWatermark(JSON.stringify({ enabled: true, opacity: 500, position: 'nowhere', color: 'red', marginPct: -1, text: 5 }))).toEqual({ ...DEFAULT_WATERMARK, enabled: true })
      localStorage.clear()
    })
  })
})
