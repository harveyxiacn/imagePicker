/** Export dialog state <-> `POST /api/export` body. Presets come from the server
 *  (`GET /api/export/presets`), the same table the AI assistant's `export` tool uses. */
import type { ExportBody, ExportFormat, ExportPreset, WatermarkBody, WatermarkPosition } from '@/api/types'

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
  watermark: WatermarkForm
}

/** The watermark section of the dialog (F9.4). `opacity` is a percentage here, 0..1 on the wire. */
export interface WatermarkForm {
  enabled: boolean
  kind: 'text' | 'image'
  text: string
  imagePath: string
  fontPath: string
  position: WatermarkPosition
  marginPct: number
  sizePct: number
  fontSizePct: number
  opacity: number
  color: string
  shadow: boolean
}

export const WM_POSITIONS: WatermarkPosition[] = ['top-left', 'top', 'top-right', 'left', 'center', 'right', 'bottom-left', 'bottom', 'bottom-right']
/** Placeholders of the watermark text. */
export const WM_TOKENS = ['{name}', '{date}', '{year}', '{camera}', '{event}', '{scene}', '{rating}']

export const DEFAULT_WATERMARK: WatermarkForm = {
  enabled: false,
  kind: 'text',
  text: '© {year}',
  imagePath: '',
  fontPath: '',
  position: 'bottom-right',
  marginPct: 2,
  sizePct: 20,
  fontSizePct: 3,
  opacity: 60,
  color: '#ffffff',
  shadow: true,
}

export const DEFAULT_FORM: ExportForm = { edge: '2048', custom: 3000, quality: 90, format: 'original', aspect: 'none', stripGps: false, stripMetadata: false, watermark: DEFAULT_WATERMARK }

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

/** The wire object of an enabled, complete watermark; `null` when off or when there is nothing to draw yet. */
export function watermarkBody(w: WatermarkForm): WatermarkBody | null {
  if (!w.enabled) return null
  const common = { position: w.position, margin_pct: w.marginPct, opacity: Math.round(w.opacity) / 100 }
  if (w.kind === 'image') {
    return w.imagePath.trim() ? { kind: 'image', image_path: w.imagePath.trim(), size_pct: w.sizePct, ...common } : null
  }
  if (!w.text.trim()) return null
  return { kind: 'text', text: w.text, ...(w.fontPath.trim() ? { font_path: w.fontPath.trim() } : {}), font_size_pct: w.fontSizePct, color: w.color, shadow: w.shadow, ...common }
}

/** Switched on but missing its text or logo: the dialog keeps Export disabled. */
export function watermarkIncomplete(w: WatermarkForm): boolean {
  return w.enabled && watermarkBody(w) === null
}

const WM_KEY = 'imagepicker.exportWatermark'

/** A stored (or hand-edited) value made into a valid form: unknown or out-of-range fields fall back to the defaults. */
export function parseWatermark(raw: string | null): WatermarkForm {
  const d = DEFAULT_WATERMARK
  let o: Record<string, unknown> = {}
  try {
    const v: unknown = raw ? JSON.parse(raw) : null
    if (v && typeof v === 'object') o = v as Record<string, unknown>
  } catch {
    /* corrupt: defaults */
  }
  const num = (v: unknown, lo: number, hi: number, dflt: number) => (typeof v === 'number' && Number.isFinite(v) && v >= lo && v <= hi ? v : dflt)
  const str = (v: unknown, dflt: string, max = 500) => (typeof v === 'string' && v.length <= max ? v : dflt)
  return {
    enabled: o.enabled === true,
    kind: o.kind === 'image' ? 'image' : 'text',
    text: str(o.text, d.text, 200),
    imagePath: str(o.imagePath, ''),
    fontPath: str(o.fontPath, ''),
    position: WM_POSITIONS.includes(o.position as WatermarkPosition) ? (o.position as WatermarkPosition) : d.position,
    marginPct: num(o.marginPct, 0, 40, d.marginPct),
    sizePct: num(o.sizePct, 1, 100, d.sizePct),
    fontSizePct: num(o.fontSizePct, 0.5, 50, d.fontSizePct),
    opacity: num(o.opacity, 0, 100, d.opacity),
    color: typeof o.color === 'string' && /^#[0-9a-fA-F]{6}$/.test(o.color) ? o.color : d.color,
    shadow: typeof o.shadow === 'boolean' ? o.shadow : d.shadow,
  }
}

/** The last-used watermark settings (localStorage may be unavailable). */
export function loadWatermark(): WatermarkForm {
  try {
    return parseWatermark(localStorage.getItem(WM_KEY))
  } catch {
    return DEFAULT_WATERMARK
  }
}

export function saveWatermark(w: WatermarkForm): void {
  try {
    localStorage.setItem(WM_KEY, JSON.stringify(w))
  } catch {
    /* ignore */
  }
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
  const watermark = watermarkBody(form.watermark)
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
    ...(watermark ? { watermark } : {}),
  }
}

const SAMPLE: Record<string, string> = { name: 'IMG_0001', seq: '0001', date: '20240229', event: 'Kyoto', scene: 'portrait', camera: 'Canon EOS R5', rating: '4' }

/** The file name the template gives a sample photo (the hint under the template field). */
export function exampleName(template: string, format: ExportFormat): string {
  const name = (template || '{name}').replace(/\{(\w+)\}/g, (all, key: string) => SAMPLE[key] ?? all)
  return `${name}.${extensionOf(format)}`
}
