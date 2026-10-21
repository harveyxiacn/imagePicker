import { useQueryClient } from '@tanstack/react-query'
import { RotateCcw } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { getDetail, noiseOf, setDetail, setNoiseReduction, setSharpenDetail, sharpenOf } from '@/lib/edit'
import { changeStack, commitLive, setLive } from '@/lib/editActions'
import { useEdit } from '@/stores/edit'
import { Section } from './Section'
import { Slider } from './Slider'

/**
 * The detail panel. Sharpening and noise reduction are one whole-image stage: the
 * noise estimate is taken from the frame itself and the sharpener runs afterwards, so
 * sharpening can never amplify noise that has just been removed. It sits after the LUT and
 * before the cheap output sharpen, which is a different control (see *Output sharpening*).
 */
export function DetailPanel() {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const stack = useEdit((s) => s.stack)
  const op = getDetail(stack)
  const sh = sharpenOf(op)
  const nr = noiseOf(op)
  const dirty = !!op

  const live = (fn: (s: typeof stack) => typeof stack) => setLive(fn(useEdit.getState().stack))
  const commit = (key: 'sharpenDetail' | 'noise') => commitLive(qc, t(`history.${key}`), `detail:${key}`)

  return (
    <Section
      id="detail"
      title={t('edit.detail')}
      defaultCollapsed
      right={
        <button
          className="btn btn-ghost btn-icon !h-6 !w-6"
          disabled={!dirty}
          title={t('edit.resetSection')}
          aria-label={t('edit.resetSection')}
          onClick={() => changeStack(qc, setDetail(useEdit.getState().stack, null), t('history.detail'))}
        >
          <RotateCcw size={12} />
        </button>
      }
    >
      <div className="mb-1 text-[11px] font-medium uppercase tracking-wide text-faint">{t('edit.sharpening')}</div>
      <div data-testid="sharpening-sliders">
        <Slider
          testId="detail-amount"
          label={t('edit.amount')}
          value={sh.amount}
          min={0}
          max={150}
          step={1}
          onLive={(x) => live((s) => setSharpenDetail(s, { amount: x }))}
          onCommit={() => commit('sharpenDetail')}
        />
        <Slider
          testId="detail-radius"
          label={t('edit.radius')}
          value={sh.radius}
          min={0.5}
          max={3}
          step={0.1}
          decimals={1}
          def={1}
          disabled={sh.amount === 0}
          onLive={(x) => live((s) => setSharpenDetail(s, { radius: x }))}
          onCommit={() => commit('sharpenDetail')}
        />
        <Slider
          testId="detail-detail"
          label={t('edit.detailAmount')}
          value={sh.detail}
          min={0}
          max={100}
          step={1}
          def={25}
          disabled={sh.amount === 0}
          onLive={(x) => live((s) => setSharpenDetail(s, { detail: x }))}
          onCommit={() => commit('sharpenDetail')}
        />
        <Slider
          testId="detail-masking"
          label={t('edit.masking')}
          value={sh.masking}
          min={0}
          max={100}
          step={1}
          disabled={sh.amount === 0}
          onLive={(x) => live((s) => setSharpenDetail(s, { masking: x }))}
          onCommit={() => commit('sharpenDetail')}
        />
      </div>

      <div className="my-1 border-t border-line/60" />
      <div className="mb-1 text-[11px] font-medium uppercase tracking-wide text-faint">{t('edit.noiseReduction')}</div>
      <div data-testid="noise-sliders">
        <Slider
          testId="noise-luminance"
          label={t('edit.noiseLuminance')}
          value={nr.luminance}
          min={0}
          max={100}
          step={1}
          onLive={(x) => live((s) => setNoiseReduction(s, { luminance: x }))}
          onCommit={() => commit('noise')}
        />
        <Slider
          testId="noise-luminance-detail"
          label={t('edit.noiseLuminanceDetail')}
          value={nr.luminance_detail}
          min={0}
          max={100}
          step={1}
          def={50}
          disabled={nr.luminance === 0}
          onLive={(x) => live((s) => setNoiseReduction(s, { luminance_detail: x }))}
          onCommit={() => commit('noise')}
        />
        <Slider
          testId="noise-luminance-contrast"
          label={t('edit.noiseLuminanceContrast')}
          value={nr.luminance_contrast}
          min={0}
          max={100}
          step={1}
          disabled={nr.luminance === 0}
          onLive={(x) => live((s) => setNoiseReduction(s, { luminance_contrast: x }))}
          onCommit={() => commit('noise')}
        />
        <Slider
          testId="noise-color"
          label={t('edit.noiseColor')}
          value={nr.color}
          min={0}
          max={100}
          step={1}
          onLive={(x) => live((s) => setNoiseReduction(s, { color: x }))}
          onCommit={() => commit('noise')}
        />
        <Slider
          testId="noise-color-detail"
          label={t('edit.noiseColorDetail')}
          value={nr.color_detail}
          min={0}
          max={100}
          step={1}
          def={50}
          disabled={nr.color === 0}
          onLive={(x) => live((s) => setNoiseReduction(s, { color_detail: x }))}
          onCommit={() => commit('noise')}
        />
      </div>
    </Section>
  )
}
