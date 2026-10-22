import { useQueryClient } from '@tanstack/react-query'
import { RotateCcw } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { VIGNETTE_STYLES } from '@/api/types'
import { getEffects, grainOf, setEffects, setGrain, setVignette, vignetteOf } from '@/lib/edit'
import { changeStack, commitLive, setLive } from '@/lib/editActions'
import { useEdit } from '@/stores/edit'
import { Section } from './Section'
import { Slider } from './Slider'

/**
 * The effects panel: a post-crop vignette and film grain. Both are per-pixel ops
 * that sit after the local adjustments and before the LUT, so a look applied on top sees
 * the vignette as part of the picture.
 */
export function EffectsPanel() {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const stack = useEdit((s) => s.stack)
  const op = getEffects(stack)
  const v = vignetteOf(op)
  const g = grainOf(op)
  const dirty = !!op

  const live = (fn: (s: typeof stack) => typeof stack) => setLive(fn(useEdit.getState().stack))
  const commit = (key: 'vignette' | 'grain') => commitLive(qc, t(`history.${key}`), `effects:${key}`)

  return (
    <Section
      id="effects"
      title={t('edit.effects')}
      defaultCollapsed
      right={
        <button
          className="btn btn-ghost btn-icon !h-6 !w-6"
          disabled={!dirty}
          title={t('edit.resetSection')}
          aria-label={t('edit.resetSection')}
          onClick={() => changeStack(qc, setEffects(useEdit.getState().stack, null), t('history.effects'))}
        >
          <RotateCcw size={12} />
        </button>
      }
    >
      <div className="mb-1 text-[11px] font-medium uppercase tracking-wide text-faint">{t('edit.vignette')}</div>
      <div data-testid="vignette-sliders">
        <Slider
          testId="vignette-amount"
          label={t('edit.vignetteAmount')}
          value={v.amount}
          min={-100}
          max={100}
          step={1}
          onLive={(x) => live((s) => setVignette(s, { amount: x }))}
          onCommit={() => commit('vignette')}
        />
        <Slider
          testId="vignette-midpoint"
          label={t('edit.vignetteMidpoint')}
          value={v.midpoint}
          min={0}
          max={100}
          step={1}
          def={50}
          disabled={v.amount === 0}
          onLive={(x) => live((s) => setVignette(s, { midpoint: x }))}
          onCommit={() => commit('vignette')}
        />
        <Slider
          testId="vignette-roundness"
          label={t('edit.vignetteRoundness')}
          value={v.roundness}
          min={-100}
          max={100}
          step={1}
          disabled={v.amount === 0}
          onLive={(x) => live((s) => setVignette(s, { roundness: x }))}
          onCommit={() => commit('vignette')}
        />
        <Slider
          testId="vignette-feather"
          label={t('edit.vignetteFeather')}
          value={v.feather}
          min={0}
          max={100}
          step={1}
          def={50}
          disabled={v.amount === 0}
          onLive={(x) => live((s) => setVignette(s, { feather: x }))}
          onCommit={() => commit('vignette')}
        />
        <Slider
          testId="vignette-highlight-contrast"
          label={t('edit.vignetteHighlightContrast')}
          value={v.highlight_contrast}
          min={0}
          max={100}
          step={1}
          disabled={v.amount === 0}
          onLive={(x) => live((s) => setVignette(s, { highlight_contrast: x }))}
          onCommit={() => commit('vignette')}
        />
      </div>
      <div className="mb-1.5 mt-1 flex gap-1" role="group" aria-label={t('edit.vignetteStyle')}>
        {VIGNETTE_STYLES.map((style) => (
          <button
            key={style}
            className="chip"
            aria-pressed={v.style === style}
            data-testid={`vignette-style-${style}`}
            disabled={v.amount === 0}
            onClick={() => {
              setLive(setVignette(useEdit.getState().stack, { style }))
              commitLive(qc, t('history.vignette'), 'effects:vignette:style')
            }}
          >
            {t(`edit.vignetteStyle_${style}`)}
          </button>
        ))}
      </div>

      <div className="my-1 border-t border-line/60" />
      <div className="mb-1 text-[11px] font-medium uppercase tracking-wide text-faint">{t('edit.grain')}</div>
      <div data-testid="grain-sliders">
        <Slider
          testId="grain-amount"
          label={t('edit.amount')}
          value={g.amount}
          min={0}
          max={100}
          step={1}
          onLive={(x) => live((s) => setGrain(s, { amount: x }))}
          onCommit={() => commit('grain')}
        />
        <Slider
          testId="grain-size"
          label={t('edit.grainSize')}
          value={g.size}
          min={0}
          max={100}
          step={1}
          def={25}
          disabled={g.amount === 0}
          onLive={(x) => live((s) => setGrain(s, { size: x }))}
          onCommit={() => commit('grain')}
        />
        <Slider
          testId="grain-frequency"
          label={t('edit.grainFrequency')}
          value={g.frequency}
          min={0}
          max={100}
          step={1}
          def={50}
          disabled={g.amount === 0}
          onLive={(x) => live((s) => setGrain(s, { frequency: x }))}
          onCommit={() => commit('grain')}
        />
      </div>
    </Section>
  )
}
