import { useTranslation } from 'react-i18next'
import type { Adjust, AdjustKey } from '@/api/types'
import { SLIDER_SPEC } from '@/lib/edit'
import { Slider } from './Slider'

const TRACKS: Partial<Record<AdjustKey, string>> = {
  temp: 'linear-gradient(to right, #3a7bd5, #cfd8e6 50%, #f0b429)',
  tint: 'linear-gradient(to right, #3fb950, #cfcfcf 50%, #d36bb0)',
}

interface Props {
  keys: AdjustKey[]
  adjust: Adjust
  onSet: (key: AdjustKey, value: number) => void
  onCommit: (key: AdjustKey) => void
  /** keys to highlight (AI apply) */
  flash?: string[]
  prefix?: string
}

/** A list of Lightroom-style sliders bound to an `Adjust` block (global or local). */
export function AdjustSliders({ keys, adjust, onSet, onCommit, flash, prefix = 'adj' }: Props) {
  const { t } = useTranslation()
  return (
    <>
      {keys.map((key) => {
        const spec = SLIDER_SPEC[key]
        return (
          <Slider
            key={key}
            testId={`${prefix}-${key}`}
            label={t(`adjust.${key}`)}
            value={adjust[key] ?? 0}
            min={spec.min}
            max={spec.max}
            step={spec.step}
            decimals={spec.decimals}
            track={TRACKS[key]}
            flash={flash?.includes(key)}
            onLive={(v) => onSet(key, v)}
            onCommit={() => onCommit(key)}
          />
        )
      })}
    </>
  )
}
