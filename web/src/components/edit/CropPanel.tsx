import { useQueryClient } from '@tanstack/react-query'
import { Crop as CropIcon, Loader2, RotateCcw, ScanLine, Sparkles } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { AutoCropKind, Photo } from '@/api/types'
import { fmtDeg, runAutoCrop } from '@/lib/autoCrop'
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
  const [busy, setBusy] = useState<AutoCropKind | null>(null)

  const auto = async (kind: AutoCropKind) => {
    setBusy(kind)
    try {
      await runAutoCrop(qc, kind, imgAspect, {
        label: t(kind === 'straighten' ? 'history.autoStraighten' : 'history.autoCrop'),
        done: (res) => (kind === 'straighten' ? t('edit.straightened', { deg: fmtDeg(res.crop?.angle ?? 0) }) : t('edit.cropSuggested')),
        level: t('edit.alreadyLevel'),
        none: t('edit.noTilt'),
      })
    } finally {
      setBusy(null)
    }
  }

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
      <div className="mt-2 flex flex-wrap gap-1.5" data-testid="auto-crop-buttons">
        <button className="btn btn-ai" disabled={busy !== null} title={t('edit.autoStraightenTip')} onClick={() => void auto('straighten')} data-testid="auto-straighten">
          {busy === 'straighten' ? <Loader2 size={13} className="animate-spin" /> : <ScanLine size={13} />}
          {t('edit.autoStraighten')}
        </button>
        <button className="btn btn-ai" disabled={busy !== null} title={t('edit.autoCropTip')} onClick={() => void auto('compose')} data-testid="auto-crop">
          {busy === 'compose' ? <Loader2 size={13} className="animate-spin" /> : <Sparkles size={13} />}
          {t('edit.autoCrop')}
        </button>
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
