import { describe, expect, it } from 'vitest'
import type { ExportPreset } from '@/api/types'
import { applyPreset, aspectOf, DEFAULT_FORM, exampleName, exportBody, extensionOf, usesQuality } from './exportOptions'

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
})
