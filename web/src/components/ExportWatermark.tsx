import { AlertTriangle, FileImage, Type } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { fileBaseName } from '@/lib/format'
import { WM_POSITIONS, WM_TOKENS, type WatermarkForm } from '@/lib/exportOptions'
import { FilePicker } from './FilePicker'

interface Props {
  value: WatermarkForm
  onChange: (patch: Partial<WatermarkForm>) => void
  /** collage mode: the mark goes on the finished collage, nothing is copied as it is */
  collage?: boolean
}

/** The "Watermark" section of the export dialog (PRD F9.4). */
export function ExportWatermark({ value: w, onChange, collage }: Props) {
  const { t } = useTranslation()
  const [picking, setPicking] = useState<'image' | 'font' | null>(null)
  const range = (id: string, label: string, v: number, min: number, max: number, step: number, unit: string, key: keyof WatermarkForm) => (
    <div className="flex items-center gap-2">
      <label htmlFor={id} className="w-28 shrink-0 text-muted">
        {label}
      </label>
      <input id={id} type="range" className="flex-1" min={min} max={max} step={step} value={v} onChange={(e) => onChange({ [key]: Number(e.target.value) })} />
      <span className="tnum w-14 text-right">
        {v}
        {unit}
      </span>
    </div>
  )
  return (
    <div className="flex flex-col gap-2" data-testid="export-watermark">
      <label className="flex items-center gap-1.5">
        <input type="checkbox" checked={w.enabled} onChange={(e) => onChange({ enabled: e.target.checked })} data-testid="export-watermark-enabled" />
        {t('export.wm')}
      </label>
      {w.enabled && (
        <div className="flex flex-col gap-2 rounded-control border border-line p-2">
          {!collage && (
            <div className="flex items-start gap-1.5 rounded-control bg-panel p-2 text-xs text-muted" data-testid="export-watermark-hint">
              <AlertTriangle size={13} className="mt-0.5 shrink-0" />
              <span>{t('export.wmReencode')}</span>
            </div>
          )}
          <div className="flex gap-1.5" role="group" aria-label={t('export.wm')}>
            <button className="chip" aria-pressed={w.kind === 'text'} onClick={() => onChange({ kind: 'text' })} data-testid="export-watermark-kind-text">
              <Type size={13} /> {t('export.wmText')}
            </button>
            <button className="chip" aria-pressed={w.kind === 'image'} onClick={() => onChange({ kind: 'image' })} data-testid="export-watermark-kind-image">
              <FileImage size={13} /> {t('export.wmImage')}
            </button>
          </div>
          {w.kind === 'text' ? (
            <>
              <div className="flex flex-col gap-1">
                <label htmlFor="wm-text" className="text-muted">
                  {t('export.wmTextLabel')}
                </label>
                <input id="wm-text" className="field" value={w.text} maxLength={200} onChange={(e) => onChange({ text: e.target.value })} data-testid="export-watermark-text" />
                <div className="text-xs text-faint">{t('export.wmTextHint', { tokens: WM_TOKENS.join(' ') })}</div>
              </div>
              <div className="flex items-center gap-2">
                <label htmlFor="wm-color" className="w-28 shrink-0 text-muted">
                  {t('export.wmColor')}
                </label>
                <input id="wm-color" type="color" value={w.color} onChange={(e) => onChange({ color: e.target.value })} />
                <label className="ml-4 flex items-center gap-1.5">
                  <input type="checkbox" checked={w.shadow} onChange={(e) => onChange({ shadow: e.target.checked })} />
                  {t('export.wmShadow')}
                </label>
              </div>
              {range('wm-font-size', t('export.wmFontSize'), w.fontSizePct, 0.5, 20, 0.5, '%', 'fontSizePct')}
              <div className="flex items-center gap-2">
                <span className="w-28 shrink-0 text-muted">{t('export.wmFont')}</span>
                <span className="flex-1 truncate font-mono text-xs" title={w.fontPath}>
                  {w.fontPath ? fileBaseName(w.fontPath) : t('export.wmFontDefault')}
                </span>
                <button className="btn" onClick={() => setPicking('font')}>
                  {t('export.wmChoose')}
                </button>
                {w.fontPath && (
                  <button className="btn" onClick={() => onChange({ fontPath: '' })}>
                    {t('export.wmClear')}
                  </button>
                )}
              </div>
              <div className="text-xs text-faint">{t('export.wmFontHint')}</div>
            </>
          ) : (
            <>
              <div className="flex items-center gap-2">
                <span className="w-28 shrink-0 text-muted">{t('export.wmLogo')}</span>
                <span className="flex-1 truncate font-mono text-xs" title={w.imagePath} data-testid="export-watermark-logo">
                  {w.imagePath ? fileBaseName(w.imagePath) : t('export.wmNoLogo')}
                </span>
                <button className="btn" onClick={() => setPicking('image')}>
                  {t('export.wmChoose')}
                </button>
              </div>
              {range('wm-size', t('export.wmSize'), w.sizePct, 1, 100, 1, '%', 'sizePct')}
            </>
          )}
          {range('wm-opacity', t('export.wmOpacity'), w.opacity, 0, 100, 5, '%', 'opacity')}
          {range('wm-margin', t('export.wmMargin'), w.marginPct, 0, 20, 0.5, '%', 'marginPct')}
          <div className="flex items-center gap-2">
            <span className="w-28 shrink-0 text-muted">{t('export.wmPosition')}</span>
            <div className="grid grid-cols-3 gap-1" role="group" aria-label={t('export.wmPosition')}>
              {WM_POSITIONS.map((p) => (
                <button key={p} className="chip h-7 w-9 justify-center" aria-pressed={w.position === p} aria-label={t(`export.wmPos_${p.replace('-', '_')}`)} title={t(`export.wmPos_${p.replace('-', '_')}`)} onClick={() => onChange({ position: p })} data-testid={`export-watermark-pos-${p}`}>
                  <span aria-hidden="true">{w.position === p ? '●' : '○'}</span>
                </button>
              ))}
            </div>
          </div>
          <div className="text-xs text-faint">{t('export.wmRelative')}</div>
        </div>
      )}
      <FilePicker
        open={picking !== null}
        onOpenChange={(o) => !o && setPicking(null)}
        exts={picking === 'font' ? ['ttf', 'otf'] : ['png', 'webp', 'jpg', 'jpeg']}
        title={picking === 'font' ? t('export.wmPickFont') : t('export.wmPickLogo')}
        onSelect={(p) => onChange(picking === 'font' ? { fontPath: p } : { imagePath: p })}
      />
    </div>
  )
}
