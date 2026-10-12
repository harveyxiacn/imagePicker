/** Export dialog state <-> `POST /api/export` body. Presets come from the server
 *  (`GET /api/export/presets`), the same table the AI assistant's `export` tool uses. */
import type { ExportBody, ExportFormat, ExportPreset } from '@/api/types'

export type EdgeChoice = 'original' | '2048' | '4096' | 'custom'

/** What the export dialog edits. `aspect` is `'none'` or `'w:h'`. */
export interface ExportForm {
  edge: EdgeChoice
  custom: number
  quality: number
  format: ExportFormat
  aspect: string
  stripGps: boolean
  stripMetadata: boolean
}

export const DEFAULT_FORM: ExportForm = { edge: '2048', custom: 3000, quality: 90, format: 'original', aspect: 'none', stripGps: false, stripMetadata: false }

export const FORMATS: ExportFormat[] = ['original', 'jpeg', 'png', 'tiff', 'webp']
export const ASPECTS = ['none', '3:4', '4:5', '1:1', '2:3', '16:9']

/** Placeholders of the name template (see docs/api-contract-m5.md F). */
export const NAME_TOKENS = ['{name}', '{seq}', '{date}', '{event}', '{scene}', '{camera}', '{rating}']

/** The form with a server preset applied (scope, destination and template are left alone). */
export function applyPreset(form: ExportForm, p: ExportPreset): ExportForm {
  const edge: Pick<ExportForm, 'edge' | 'custom'> =
    p.long_edge === null ? { edge: 'original', custom: form.custom } : p.long_edge === 2048 || p.long_edge === 4096 ? { edge: String(p.long_edge) as EdgeChoice, custom: form.custom } : { edge: 'custom', custom: p.long_edge }
  return { ...form, ...edge, quality: p.quality, format: p.format, aspect: p.aspect ? `${p.aspect[0]}:${p.aspect[1]}` : 'none', stripGps: p.strip_gps, stripMetadata: false }
}

export function longEdgeOf(form: ExportForm): number | null {
  return form.edge === 'original' ? null : form.edge === 'custom' ? form.custom : Number(form.edge)
}

/** `'3:4'` -> `[3, 4]`; `'none'` or anything malformed -> `null`. */
export function aspectOf(choice: string): [number, number] | null {
  const m = /^(\d+):(\d+)$/.exec(choice)
  if (!m) return null
  const [w, h] = [Number(m[1]), Number(m[2])]
  return w > 0 && h > 0 ? [w, h] : null
}

/** Only JPEG output has a quality setting (PNG, TIFF and WebP are lossless). */
export function usesQuality(format: ExportFormat): boolean {
  return format === 'original' || format === 'jpeg'
}

/** Extension of re-encoded outputs. */
export function extensionOf(format: ExportFormat): string {
  return format === 'png' ? 'png' : format === 'tiff' ? 'tif' : format === 'webp' ? 'webp' : 'jpg'
}

export function exportBody(form: ExportForm, target: { ids: number[] } | { folders: Record<string, number[]> }, dest: string, template: string, upscale: 0 | 2 | 4): ExportBody {
  return {
    ...target,
    dest,
    long_edge: longEdgeOf(form),
    quality: form.quality,
    name_template: template || '{name}',
    format: form.format,
    aspect: aspectOf(form.aspect),
    strip_gps: form.stripGps || form.stripMetadata,
    strip_metadata: form.stripMetadata,
    ...(upscale ? { upscale } : {}),
  }
}

const SAMPLE: Record<string, string> = { name: 'IMG_0001', seq: '0001', date: '20240229', event: 'Kyoto', scene: 'portrait', camera: 'Canon EOS R5', rating: '4' }

/** The file name the template gives a sample photo (the hint under the template field). */
export function exampleName(template: string, format: ExportFormat): string {
  const name = (template || '{name}').replace(/\{(\w+)\}/g, (all, key: string) => SAMPLE[key] ?? all)
  return `${name}.${extensionOf(format)}`
}
