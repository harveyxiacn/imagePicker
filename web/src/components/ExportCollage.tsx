import { useTranslation } from 'react-i18next'
import { thumbUrl } from '@/api/client'
import type { Photo } from '@/api/types'
import { CANVAS_PRESETS, MAX_SIDE, MAX_SPACING, MIN_SIDE } from '@/lib/collageLayout'
import { canvasOf, COLLAGE_LAYOUTS, isColor, type CollageForm } from '@/lib/collageOptions'
import type { Cell } from '@/lib/collageLayout'

interface Props {
  value: CollageForm
  onChange: (patch: Partial<CollageForm>) => void
  /** the photos in layout order (`null` entries: not loaded, drawn as plain boxes) */
  photos: (Pick<Photo, 'id' | 'thumb_version' | 'file_name'> | null)[]
  cells: Cell[] | null
  problem: 'count' | 'size' | 'spacing' | 'tooSmall' | null
}

/** The "Collage" settings of the export dialog (PRD F9.5) with a live layout preview. */
export function ExportCollage({ value: f, onChange, photos, cells, problem }: Props) {
  const { t } = useTranslation()
  const { width, height } = canvasOf(f)
  const num = (id: string, label: string, v: number, min: number, max: number, key: keyof CollageForm) => (
    <div className="flex items-center gap-2">
      <label htmlFor={id} className="w-28 shrink-0 text-muted">
        {label}
      </label>
      <input id={id} type="number" className="field w-28" min={min} max={max} value={v} onChange={(e) => onChange({ [key]: Math.max(0, Math.round(Number(e.target.value) || 0)) })} />
    </div>
  )
  // the preview box keeps the canvas shape inside 360 x 260 px
  const scale = Math.min(360 / width, 260 / height)
  const pct = (v: number, total: number) => `${(v / total) * 100}%`
  return (
    <div className="flex flex-col gap-3" data-testid="export-collage">
      <div className="flex flex-col gap-1">
        <span className="text-muted">{t('collage.layout')}</span>
        <div className="flex flex-wrap gap-1.5" role="group" aria-label={t('collage.layout')}>
          {COLLAGE_LAYOUTS.map((l) => (
            <button key={l} className="chip" aria-pressed={f.layout === l} onClick={() => onChange({ layout: l })} data-testid={`collage-layout-${l}`}>
              {t(`collage.layout_${l}`)}
            </button>
          ))}
        </div>
        <div className="text-xs text-faint">{t(`collage.layoutHint_${f.layout}`)}</div>
      </div>

      <div className="flex flex-col gap-1">
        <label htmlFor="col-canvas" className="text-muted">
          {t('collage.canvas')}
        </label>
        <div className="flex flex-wrap items-center gap-2">
          <select id="col-canvas" className="field" value={f.preset} onChange={(e) => onChange({ preset: e.target.value })} data-testid="collage-canvas">
            {CANVAS_PRESETS.map((p) => (
              <option key={p.id} value={p.id}>
                {t(`collage.canvas_${p.id.replace(':', 'x')}`)}
              </option>
            ))}
            <option value="custom">{t('collage.canvas_custom')}</option>
          </select>
          {f.preset === 'custom' && (
            <>
              <input type="number" className="field w-24" min={MIN_SIDE} max={MAX_SIDE} value={f.customWidth} aria-label={t('collage.width')} onChange={(e) => onChange({ customWidth: Math.round(Number(e.target.value) || 0) })} />
              <span className="text-muted">×</span>
              <input type="number" className="field w-24" min={MIN_SIDE} max={MAX_SIDE} value={f.customHeight} aria-label={t('collage.height')} onChange={(e) => onChange({ customHeight: Math.round(Number(e.target.value) || 0) })} />
            </>
          )}
        </div>
      </div>

      <div className="flex flex-col gap-1.5">
        {num('col-gap', t('collage.gap'), f.gap, 0, MAX_SPACING, 'gap')}
        {num('col-margin', t('collage.margin'), f.margin, 0, MAX_SPACING, 'margin')}
        <div className="flex items-center gap-2">
          <label htmlFor="col-bg" className="w-28 shrink-0 text-muted">
            {t('collage.background')}
          </label>
          <input id="col-bg" type="color" className="h-7 w-10 cursor-pointer rounded-control border border-line bg-transparent" value={isColor(f.background) && f.background.length === 7 ? f.background : '#ffffff'} onChange={(e) => onChange({ background: e.target.value })} data-testid="collage-bg" />
        </div>
      </div>

      <div className="flex flex-col gap-1">
        <span className="text-muted">{t('collage.fit')}</span>
        <div className="flex flex-wrap gap-1.5" role="group" aria-label={t('collage.fit')}>
          {(['cover', 'contain'] as const).map((v) => (
            <button key={v} className="chip" aria-pressed={f.fit === v} onClick={() => onChange({ fit: v })} data-testid={`collage-fit-${v}`}>
              {t(`collage.fit_${v}`)}
            </button>
          ))}
        </div>
      </div>

      {f.layout === 'contact' && (
        <div className="flex flex-col gap-1" data-testid="collage-captions">
          <span className="text-muted">{t('collage.captions')}</span>
          <label className="flex items-center gap-1.5">
            <input type="checkbox" checked={f.captionName} onChange={(e) => onChange({ captionName: e.target.checked })} />
            {t('collage.captionName')}
          </label>
          <label className="flex items-center gap-1.5">
            <input type="checkbox" checked={f.captionRating} onChange={(e) => onChange({ captionRating: e.target.checked })} />
            {t('collage.captionRating')}
          </label>
        </div>
      )}

      <div className="flex flex-col gap-1">
        <span className="text-muted">
          {t('collage.preview')} <span className="tnum text-faint">{width} × {height}</span>
        </span>
        {problem ? (
          <div className="rounded-control bg-warning/10 p-2 text-xs text-warning" role="alert" data-testid="collage-problem">
            {t(`collage.problem_${problem}`)}
          </div>
        ) : (
          cells && (
            <div className="relative mx-auto overflow-hidden rounded-control border border-line" style={{ width: Math.round(width * scale), height: Math.round(height * scale), background: f.background }} data-testid="collage-preview">
              {cells.map((c, i) => {
                const p = photos[i]
                return (
                  <div key={i}>
                    <div className="absolute overflow-hidden bg-panel" style={{ left: pct(c.photo.x, width), top: pct(c.photo.y, height), width: pct(c.photo.w, width), height: pct(c.photo.h, height) }} data-testid="collage-cell">
                      {p && <img src={thumbUrl(p)} alt="" className={`h-full w-full ${f.fit === 'cover' ? 'object-cover' : 'object-contain'}`} draggable={false} />}
                    </div>
                    {c.caption && <div className="absolute rounded-sm bg-black/30" style={{ left: pct(c.caption.x + c.caption.w * 0.02, width), top: pct(c.caption.y + c.caption.h * 0.3, height), width: pct(c.caption.w * 0.6, width), height: pct(c.caption.h * 0.4, height) }} />}
                  </div>
                )
              })}
            </div>
          )
        )}
        <div className="text-xs text-faint">{t('collage.previewHint')}</div>
      </div>
    </div>
  )
}
