import { useQueryClient } from '@tanstack/react-query'
import { Loader2, Mountain, RotateCcw, Sparkles, User, Wand2, X } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { AdjustKey, AutoMode } from '@/api/types'
import { undo } from '@/lib/actions'
import { ADJUST_NUMERIC_KEYS } from '@/api/types'
import { autoApply, changeStack, commitLive, setLive } from '@/lib/editActions'
import { getAdjust, getSharpen, setGlobalField, setSharpen, updateGlobal } from '@/lib/edit'
import { useEdit } from '@/stores/edit'
import { AdjustSliders } from './AdjustSliders'
import { Section } from './Section'
import { Slider } from './Slider'

const AUTO_MODES: { mode: AutoMode; icon: typeof Wand2 }[] = [
  { mode: 'auto', icon: Wand2 },
  { mode: 'portrait', icon: User },
  { mode: 'landscape', icon: Mountain },
]

/** ✨ AI one-click: calls /auto, merges into the global op, shows an animated diff with undo. */
export function AiPanel() {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const [busy, setBusy] = useState<AutoMode | null>(null)
  const applied = useEdit((s) => s.autoApplied)
  const setApplied = useEdit((s) => s.setAutoApplied)

  const run = async (mode: AutoMode) => {
    setBusy(mode)
    try {
      await autoApply(qc, mode, t('history.auto', { mode: t(`edit.auto_${mode}`) }))
    } finally {
      setBusy(null)
    }
  }

  return (
    <Section id="ai" ai icon={<Sparkles size={13} />} title={t('edit.ai')}>
      <div className="flex flex-wrap gap-1.5" data-testid="ai-buttons">
        {AUTO_MODES.map(({ mode, icon: Icon }) => (
          <button key={mode} className="btn btn-ai" disabled={busy !== null} onClick={() => void run(mode)} data-testid={`auto-${mode}`}>
            {busy === mode ? <Loader2 size={13} className="animate-spin" /> : <Icon size={13} />}
            {t(`edit.auto_${mode}`)}
          </button>
        ))}
        <button className="btn" disabled title={t('common.soon')}>
          {t('edit.matchRef')}
        </button>
      </div>
      {applied && (
        <div key={applied.nonce} className="anim-pop mt-2 rounded-card border border-ai/40 bg-ai/10 p-2" data-testid="auto-applied">
          <div className="mb-1.5 flex items-center gap-1 text-xs text-ai">
            <Sparkles size={12} />
            <span className="flex-1 font-medium">{applied.diff.length ? t('edit.applied', { n: applied.diff.length }) : t('edit.appliedNone')}</span>
            <button
              className="btn btn-ghost !h-5 px-1.5 text-xs text-ai"
              data-testid="auto-undo"
              onClick={() => {
                void undo(qc)
                setApplied(null)
              }}
            >
              <RotateCcw size={11} />
              {t('edit.undo')}
            </button>
            <button className="btn btn-ghost btn-icon !h-5 !w-5" aria-label={t('common.close')} onClick={() => setApplied(null)}>
              <X size={11} />
            </button>
          </div>
          <div className="flex flex-wrap gap-1">
            {applied.diff.map((d, i) => (
              <span key={d.key} className="anim-chip tnum rounded bg-bg/60 px-1.5 py-0.5 text-[11px]" style={{ animationDelay: `${i * 45}ms` }}>
                <span className="text-muted">{t(`adjust.${d.key}`)} </span>
                <span className="text-faint">{d.from.toFixed(d.key === 'exposure' ? 2 : 0)}</span>
                <span className="text-faint"> → </span>
                <span className="text-ai">{(d.to > 0 ? '+' : '') + d.to.toFixed(d.key === 'exposure' ? 2 : 0)}</span>
              </span>
            ))}
          </div>
        </div>
      )}
    </Section>
  )
}

const LIGHT: AdjustKey[] = ['exposure', 'contrast', 'highlights', 'shadows', 'whites', 'blacks']
const COLOR: AdjustKey[] = ['temp', 'tint', 'vibrance', 'saturation']
const PRESENCE: AdjustKey[] = ['clarity', 'dehaze']

/** Basic: 12 Lightroom-like sliders on the global op. */
export function BasicPanel() {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const stack = useEdit((s) => s.stack)
  const flash = useEdit((s) => s.flash)
  const adjust = getAdjust(stack)

  const onSet = (key: AdjustKey, v: number) => setLive(setGlobalField(useEdit.getState().stack, key, v))
  const onCommit = (key: AdjustKey) => commitLive(qc, t('history.adjust', { name: t(`adjust.${key}`) }), `adjust:${key}`)
  const dirty = ADJUST_NUMERIC_KEYS.some((k) => (adjust[k] ?? 0) !== 0)

  return (
    <Section
      id="basic"
      title={t('edit.basic')}
      right={
        <button
          className="btn btn-ghost btn-icon !h-6 !w-6"
          disabled={!dirty}
          title={t('edit.resetSection')}
          aria-label={t('edit.resetSection')}
          onClick={() =>
            changeStack(
              qc,
              updateGlobal(useEdit.getState().stack, (a) => {
                const next = { ...a }
                for (const k of ADJUST_NUMERIC_KEYS) delete next[k]
                return next
              }),
              t('history.resetBasic'),
            )
          }
        >
          <RotateCcw size={12} />
        </button>
      }
    >
      <div className="flex flex-col" data-testid="basic-sliders">
        <AdjustSliders keys={LIGHT} adjust={adjust} onSet={onSet} onCommit={onCommit} flash={flash} />
        <div className="my-1 border-t border-line/60" />
        <AdjustSliders keys={COLOR} adjust={adjust} onSet={onSet} onCommit={onCommit} flash={flash} />
        <div className="my-1 border-t border-line/60" />
        <AdjustSliders keys={PRESENCE} adjust={adjust} onSet={onSet} onCommit={onCommit} flash={flash} />
      </div>
    </Section>
  )
}

/** Output sharpening (applied last in the pipeline). */
export function SharpenPanel() {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const stack = useEdit((s) => s.stack)
  return (
    <Section id="sharpen" title={t('edit.sharpen')} defaultCollapsed>
      <Slider
        testId="sharpen-amount"
        label={t('edit.amount')}
        value={getSharpen(stack)}
        min={0}
        max={100}
        step={1}
        onLive={(v) => setLive(setSharpen(useEdit.getState().stack, v))}
        onCommit={() => commitLive(qc, t('history.sharpen'), 'sharpen')}
      />
    </Section>
  )
}
