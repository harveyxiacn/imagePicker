/** Collage mode of the export dialog (PRD F9.5): form state <-> `POST /api/export/collage` body. */
import type { CollageBody, ExportFormat, WatermarkBody } from '@/api/types'
import { CANVAS_PRESETS, captionHeight, collageProblem, computeLayout, MAX_PHOTOS, MIN_PHOTOS, type Cell, type CollageFit, type CollageLayoutKind } from './collageLayout'

export interface CollageForm {
  layout: CollageLayoutKind
  /** a `CANVAS_PRESETS` id or `custom` */
  preset: string
  customWidth: number
  customHeight: number
  gap: number
  margin: number
  background: string
  fit: CollageFit
  captionName: boolean
  captionRating: boolean
}

export const DEFAULT_COLLAGE: CollageForm = { layout: 'grid', preset: '4:5', customWidth: 2000, customHeight: 1500, gap: 16, margin: 16, background: '#ffffff', fit: 'cover', captionName: true, captionRating: true }

export const COLLAGE_LAYOUTS: CollageLayoutKind[] = ['grid', 'story', 'contact']
export const COLLAGE_TEMPLATE = 'collage'

/** Canvas size in pixels of the form. */
export function canvasOf(f: CollageForm): { width: number; height: number } {
  const p = CANVAS_PRESETS.find((x) => x.id === f.preset)
  return p ? { width: p.width, height: p.height } : { width: f.customWidth, height: f.customHeight }
}

/** The caption band height of the form (0 when there is none: other layouts, or both captions off). */
export function captionOf(f: CollageForm): number {
  if (f.layout !== 'contact' || !(f.captionName || f.captionRating)) return 0
  const { width, height } = canvasOf(f)
  return captionHeight(width, height)
}

/** What the live preview draws: the cells for the photos' aspect ratios (in order), `null` when the canvas is too small. */
export function previewCells(f: CollageForm, aspects: number[]): Cell[] | null {
  const { width, height } = canvasOf(f)
  if (![width, height].every((v) => Number.isFinite(v) && v > 0)) return null
  return computeLayout(f.layout, width, height, f.gap, f.margin, captionOf(f), aspects)
}

/** Whether the collage can be exported with these photos: `null` = yes, else the reason (i18n key `export.collageProblem_<key>`). */
export function problemOf(f: CollageForm, aspects: number[]): ReturnType<typeof collageProblem> {
  const { width, height } = canvasOf(f)
  return collageProblem(aspects.length, width, height, f.gap, f.margin, aspects.length >= MIN_PHOTOS && aspects.length <= MAX_PHOTOS ? previewCells(f, aspects) : [])
}

/** A colour the server accepts (`#rgb` / `#rrggbb`). */
export const isColor = (s: string) => /^#([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/.test(s)

export function collageBody(f: CollageForm, ids: number[], dest: string, o: { format: ExportFormat; quality: number; template: string; watermark: WatermarkBody | null }): CollageBody {
  const { width, height } = canvasOf(f)
  return {
    ids,
    dest,
    layout: f.layout,
    width,
    height,
    gap: f.gap,
    margin: f.margin,
    background: f.background,
    fit: f.fit,
    caption_name: f.captionName,
    caption_rating: f.captionRating,
    // there is no original format to keep: "original" means JPEG here
    format: o.format === 'original' ? 'jpeg' : o.format,
    quality: o.quality,
    name_template: o.template.trim() || COLLAGE_TEMPLATE,
    ...(o.watermark ? { watermark: o.watermark } : {}),
  }
}
