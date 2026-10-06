import { useQueryClient } from '@tanstack/react-query'
import { Crop as CropIcon, RotateCcw } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import type { Photo } from '@/api/types'
import { ASPECTS, aspectRatio, fitAspect, FULL_RECT, getCrop, setCrop } from '@/lib/edit'
import { changeStack, commitLive, setLive } from '@/lib/editActions'
import { useEdit } from '@/stores/edit'
import { Section } from './Section'
import { Slider } from './Slider'

/** Crop & straighten: aspect presets, angle slider, `R` toggles the on-canvas crop tool. */
export function CropPanel({ photo }: { photo: Photo }) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const stack = useEdit((s) => s.stack)
  const cropMode = useEdit((s) => s.cropMode)
  const crop = getCrop(stack)
  const imgAspect = (photo.width ?? 3) / (photo.height ?? 2)
  const aspect = crop?.aspect ?? 'original'

  const pickAspect = (id: string) => {
    const st = useEdit.getState()
    if (!st.cropMode) st.setCropMode(true)
    const ratio = aspectRatio(id, imgAspect)
    const base = getCrop(st.stack)?.rect ?? FULL_RECT
    const rect = ratio ? fitAspect(id === 'original' ? FULL_RECT : base, ratio, imgAspect) : base
    changeStack(qc, setCrop(st.stack, { rect, aspect: id }), t('history.crop'))
  }

  return (
    <Section
      id="crop"
      title={t('edit.crop')}
      defaultCollapsed
      right={
        <button
          className="chip"
          aria-pressed={cropMode}
          onClick={() => useEdit.getState().setCropMode(!cropMode)}
          title={`${t('edit.cropTool')} (R)`}
          data-testid="crop-toggle"
        >
          <CropIcon size={11} />R
        </button>
      }
    >
      <div className="flex flex-wrap gap-1.5" data-testid="aspect-chips">
        {ASPECTS.map((id) => (
          <button key={id} className="chip" aria-pressed={aspect === id} onClick={() => pickAspect(id)} data-testid={`aspect-${id}`}>
            {id === 'original' || id === 'free' ? t(`edit.aspect_${id}`) : id}
          </button>
        ))}
      </div>
      <div className="mt-2">
        <Slider
          testId="crop-angle"
          label={t('edit.angle')}
          value={crop?.angle ?? 0}
          min={-45}
          max={45}
          step={0.1}
          decimals={1}
          unit="°"
          onLive={(v) => setLive(setCrop(useEdit.getState().stack, { angle: v }))}
          onCommit={() => commitLive(qc, t('history.straighten'), 'crop:angle')}
        />
      </div>
      <div className="mt-1 flex gap-1.5">
        <button className="btn" disabled={!crop} onClick={() => changeStack(qc, setCrop(useEdit.getState().stack, null), t('history.resetCrop'))} data-testid="crop-reset">
          <RotateCcw size={12} />
          {t('edit.resetCrop')}
        </button>
        {cropMode && (
          <button className="btn btn-primary" onClick={() => useEdit.getState().setCropMode(false)} data-testid="crop-done">
            {t('edit.cropDone')}
          </button>
        )}
      </div>
    </Section>
  )
}
