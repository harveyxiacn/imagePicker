import { useQueryClient } from '@tanstack/react-query'
import { ChevronDown, ChevronUp, Eclipse, Loader2, Mountain, RotateCcw, ScanFace, Sparkles, User, Wand2 } from 'lucide-react'
import { useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { usePresets } from '@/api/queries'
import { ADJUST_NUMERIC_KEYS, type AdjustKey, type AutoMode, type Photo, type Preset } from '@/api/types'
import { applyPreset, getAdjust, SLIDER_SPEC, setGlobalField } from '@/lib/edit'
import { autoApply, changeStack, commitLive, resetAll, setLive } from '@/lib/editActions'
import { haptic } from '@/lib/haptics'
import { useEdit } from '@/stores/edit'
import { PortraitPanel } from '../edit/PortraitPanel'

const AUTO_MODES: { mode: AutoMode; icon: typeof Wand2 }[] = [
  { mode: 'auto', icon: Wand2 },
  { mode: 'portrait', icon: User },
  { mode: 'landscape', icon: Mountain },
]

type Tab = 'adjust' | 'portrait'

/** Hold to see the original; release to return to the edit. */
export function HoldCompare() {
  const { t } = useTranslation()
  const compare = useEdit((s) => s.compare)
  const on = () => {
    const st = useEdit.getState()
    if (!st.cropMode) st.setCompare('original')
    haptic('tick')
  }
  const off = () => {
    const st = useEdit.getState()
    if (st.compare === 'original') st.setCompare('off')
  }
  return (
    <button
      className={`btn absolute bottom-3 left-3 z-20 !h-12 touch-none select-none rounded-full border-white/20 bg-black/60 px-4 text-white backdrop-blur ${compare === 'original' ? '!border-accent' : ''}`}
      onPointerDown={(e) => {
        e.currentTarget.setPointerCapture(e.pointerId)
        on()
      }}
      onPointerUp={off}
      onPointerCancel={off}
      onContextMenu={(e) => e.preventDefault()}
      aria-pressed={compare === 'original'}
      data-testid="hold-compare"
    >
      <Eclipse size={18} />
      {compare === 'original' ? t('mobile.edit.original') : t('mobile.edit.holdCompare')}
    </button>
  )
}

/**
 * Mobile edit drawer (doc 08 section 5): AI one-click + presets first, a row of parameter chips,
 * and a single big slider for the selected parameter. A second tab holds the per-person portrait panel.
 */
export function MobileEditDrawer({ photo }: { photo: Photo }) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const stack = useEdit((s) => s.stack)
  const presets = usePresets()
  const [tab, setTab] = useState<Tab>('adjust')
  const [key, setKey] = useState<AdjustKey>('exposure')
  const [open, setOpen] = useState(true)
  const [busy, setBusy] = useState<AutoMode | null>(null)
  const dragging = useRef(false)

  const adjust = getAdjust(stack)
  const spec = SLIDER_SPEC[key]
  const value = adjust[key] ?? 0
  const label = t(`adjust.${key}`)
  const presetLabel = (p: Preset) => (p.builtin ? t(p.name, { defaultValue: p.id }) : p.name)

  const run = async (mode: AutoMode) => {
    setBusy(mode)
    try {
      await autoApply(qc, mode, t('history.auto', { mode: t(`edit.auto_${mode}`) }))
    } finally {
      setBusy(null)
    }
  }

  const end = () => {
    if (!dragging.current) return
    dragging.current = false
    useEdit.getState().setDragging(false)
    commitLive(qc, t('history.adjust', { name: label }), `adjust:${key}`)
  }

  return (
    <div className="shrink-0 border-t border-line bg-panel" style={{ paddingBottom: 'env(safe-area-inset-bottom, 0px)' }} data-testid="edit-drawer">
      <button className="flex w-full items-center justify-center py-1.5" onClick={() => setOpen(!open)} aria-expanded={open} aria-label={t('mobile.edit.toggle')} data-testid="drawer-toggle">
        {open ? <ChevronDown size={18} className="text-muted" /> : <ChevronUp size={18} className="text-muted" />}
      </button>

      {open && (
        <>
          {/* row 1: AI one-click + presets */}
          <div className="flex gap-2 overflow-x-auto px-3 pb-2" data-testid="drawer-ai-row">
            {AUTO_MODES.map(({ mode, icon: Icon }) => (
              <button key={mode} className="btn btn-ai !h-11 shrink-0" disabled={busy !== null} onClick={() => void run(mode)} data-testid={`m-auto-${mode}`}>
                {busy === mode ? <Loader2 size={15} className="animate-spin" /> : <Icon size={15} />}
                {t(`edit.auto_${mode}`)}
              </button>
            ))}
            <span className="mx-0.5 w-px shrink-0 self-stretch bg-line" aria-hidden />
            {(presets.data ?? []).map((p) => (
              <button
                key={p.id}
                className="btn !h-11 shrink-0"
                onClick={() => changeStack(qc, applyPreset(useEdit.getState().stack, p.stack), t('history.preset', { name: presetLabel(p) }))}
                data-testid={`m-preset-${p.id}`}
              >
                <Sparkles size={13} className="text-ai" />
                {presetLabel(p)}
              </button>
            ))}
          </div>

          {/* tabs */}
          <div className="flex gap-1 px-3 pb-1" role="tablist">
            {(['adjust', 'portrait'] as Tab[]).map((tb) => (
              <button
                key={tb}
                role="tab"
                aria-selected={tab === tb}
                className={`flex min-h-10 items-center gap-1.5 rounded-full px-4 text-sm ${tab === tb ? 'bg-accent text-accent-fg font-semibold' : 'text-muted'}`}
                onClick={() => setTab(tb)}
                data-testid={`drawer-tab-${tb}`}
              >
                {tb === 'portrait' && <ScanFace size={14} />}
                {t(tb === 'adjust' ? 'mobile.edit.adjust' : 'mobile.edit.portrait')}
              </button>
            ))}
            <button className="btn btn-ghost ml-auto !h-10" onClick={() => resetAll(qc, t('history.resetAll'))} data-testid="m-reset-all">
              <RotateCcw size={14} />
              {t('edit.resetAll')}
            </button>
          </div>

          {tab === 'adjust' ? (
            <div data-testid="drawer-adjust">
              <div className="flex gap-1.5 overflow-x-auto px-3 py-1.5" role="tablist" aria-label={t('mobile.edit.params')} data-testid="param-chips">
                {ADJUST_NUMERIC_KEYS.map((k) => {
                  const changed = (adjust[k] ?? 0) !== 0
                  return (
                    <button
                      key={k}
                      role="tab"
                      aria-selected={key === k}
                      className={`flex min-h-11 shrink-0 flex-col items-center justify-center rounded-control border px-3 text-xs ${key === k ? 'border-accent bg-accent/15 text-fg' : 'border-line text-muted'}`}
                      onClick={() => setKey(k)}
                      data-testid={`param-${k}`}
                    >
                      <span>{t(`adjust.${k}`)}</span>
                      {changed && <span className="tnum text-[10px] text-accent">{(adjust[k] ?? 0) > 0 ? '+' : ''}{(adjust[k] ?? 0).toFixed(SLIDER_SPEC[k].decimals)}</span>}
                    </button>
                  )
                })}
              </div>
              <div className="px-4 pt-1 pb-3">
                <div className="flex items-baseline justify-between">
                  <span className="text-base font-medium">{label}</span>
                  <span className="tnum text-2xl font-semibold" data-testid="big-slider-value">
                    {value > 0 ? '+' : ''}
                    {value.toFixed(spec.decimals)}
                  </span>
                </div>
                <input
                  type="range"
                  className="big-slider mt-1 w-full"
                  min={spec.min}
                  max={spec.max}
                  step={spec.step}
                  value={value}
                  aria-label={label}
                  data-testid="big-slider"
                  onPointerDown={() => {
                    dragging.current = true
                    useEdit.getState().setDragging(true)
                  }}
                  onChange={(e) => {
                    // keyboard / assistive input has no pointer-down: treat the change as a gesture too
                    dragging.current = true
                    setLive(setGlobalField(useEdit.getState().stack, key, Number(e.target.value)))
                  }}
                  onPointerUp={end}
                  onPointerCancel={end}
                  onKeyUp={end}
                  onBlur={end}
                />
                <div className="flex justify-between">
                  <span className="tnum text-xs text-faint">{spec.min}</span>
                  <button className="btn btn-ghost !h-8" disabled={value === 0} onClick={() => { setLive(setGlobalField(useEdit.getState().stack, key, 0)); dragging.current = true; end() }} data-testid="slider-reset">
                    <RotateCcw size={12} />
                    {t('edit.resetSection')}
                  </button>
                  <span className="tnum text-xs text-faint">{spec.max}</span>
                </div>
              </div>
            </div>
          ) : (
            <div className="max-h-[38dvh] overflow-y-auto overscroll-contain" data-testid="drawer-portrait">
              <PortraitPanel key={photo.id} photo={photo} />
            </div>
          )}
        </>
      )}
    </div>
  )
}
