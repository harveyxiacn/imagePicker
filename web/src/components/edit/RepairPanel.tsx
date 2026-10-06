import { useQueryClient } from '@tanstack/react-query'
import { Brush, Check, Eraser, Eye, EyeOff, Loader2, Sparkles, SmilePlus, Trash2, Undo2, Wand2, X } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { api, assetUrl } from '@/api/client'
import { useBystanders } from '@/api/queries'
import type { EnhanceOp, Photo } from '@/api/types'
import { changeStack } from '@/lib/editActions'
import { runGen } from '@/lib/gen'
import { isPatchEnabled, patchEntries, patchLabelKey, removePatch, setPatchEnabled } from '@/lib/patches'
import { clampRadius, MAX_RADIUS, MIN_RADIUS, strokesBody } from '@/lib/strokes'
import { useEdit } from '@/stores/edit'
import { useGen } from '@/stores/gen'
import { useRepair } from '@/stores/repair'
import { Section } from './Section'
import { Slider } from './Slider'

/** 修复 section of the edit panel (doc 04 3.5): bystander removal, brush erase, face restore, denoise and the patch layer list. */
export function RepairPanel({ photo }: { photo: Photo }) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const stack = useEdit((s) => s.stack)
  const stackOfThisPhoto = useEdit((s) => s.photoId === photo.id)
  const tasks = useGen((s) => s.tasks)
  const brush = useRepair((s) => s.brush)
  const radius = useRepair((s) => s.radius)
  const strokes = useRepair((s) => s.strokes)
  const bystanders = useBystanders(photo.id)
  const nBystanders = bystanders.data?.length ?? 0
  const [strength, setStrength] = useState<Record<EnhanceOp, number>>({ face_restore: 0.7, denoise: 0.6 })

  const busy = (kind: 'inpaint' | 'enhance') => Object.values(tasks).some((x) => x.kind === kind && x.photoId === photo.id && x.state === 'running')
  const inpaintBusy = busy('inpaint')
  const enhanceBusy = busy('enhance')
  // the store still holds the previous photo's stack for a moment after navigating: show nothing until it is loaded
  const layers = useMemo(() => (stackOfThisPhoto ? patchEntries(stack) : []), [stack, stackOfThisPhoto])

  // Enter applies the painted strokes while the brush is active
  const applyBrush = () => {
    const list = useRepair.getState().strokes
    if (list.length === 0 || inpaintBusy) return
    void runGen(qc, {
      kind: 'inpaint',
      sessionId: photo.session_id,
      photoId: photo.id,
      label: t('history.brushErase'),
      start: () => api.inpaint(photo.id, strokesBody(list)),
      onAccepted: () => useRepair.getState().setBrush(false),
    })
  }
  useEffect(() => {
    if (!brush) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Enter' && !document.querySelector('[role="dialog"]') && !(e.target instanceof HTMLInputElement)) {
        e.preventDefault()
        applyBrush()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [brush, inpaintBusy])

  const removeBystanders = () =>
    void runGen(qc, {
      kind: 'inpaint',
      sessionId: photo.session_id,
      photoId: photo.id,
      label: t('history.removeBystanders'),
      start: () => api.inpaint(photo.id, { bystanders: true }),
    })

  const enhance = (op: EnhanceOp) =>
    void runGen(qc, {
      kind: 'enhance',
      sessionId: photo.session_id,
      photoId: photo.id,
      label: t(op === 'denoise' ? 'history.denoise' : 'history.faceRestore'),
      start: () => api.enhance(photo.id, { op, strength: strength[op] }),
    })

  const burstOk = photo.burst_id !== null && (photo.burst_size ?? 0) > 1
  const layerName = (kind: Parameters<typeof patchLabelKey>[0], person?: number) => `${t(patchLabelKey(kind))}${person !== undefined ? ` · #${person}` : ''}`

  return (
    <Section id="repair" icon={<Eraser size={13} />} title={t('repair.title')}>
      <div className="flex flex-col gap-2.5" data-testid="repair-panel">
        <button
          type="button"
          className="btn w-full justify-start"
          disabled={nBystanders === 0 || inpaintBusy}
          onClick={removeBystanders}
          onMouseEnter={() => useRepair.getState().setHoverBystanders(true)}
          onMouseLeave={() => useRepair.getState().setHoverBystanders(false)}
          onFocus={() => useRepair.getState().setHoverBystanders(true)}
          onBlur={() => useRepair.getState().setHoverBystanders(false)}
          title={nBystanders === 0 ? t('repair.noBystanders') : t('repair.bystandersHint')}
          data-testid="repair-bystanders"
        >
          {inpaintBusy ? <Loader2 size={14} className="animate-spin" /> : <Eraser size={14} />}
          {t('repair.bystanders', { n: nBystanders })}
        </button>

        <div className="flex flex-col gap-2 rounded-control border border-line p-2">
          <button
            type="button"
            className="btn w-full justify-start"
            aria-pressed={brush}
            onClick={() => useRepair.getState().setBrush(!brush)}
            title={`${t('repair.brush')} (Shift+E)`}
            data-testid="repair-brush"
          >
            <Brush size={14} />
            {t('repair.brush')}
            <kbd className="ml-auto">Shift+E</kbd>
          </button>
          {brush && (
            <>
              <p className="text-xs text-muted">{t('repair.brushHint')}</p>
              <Slider
                label={t('repair.radius')}
                value={Math.round(radius * 1000) / 10}
                min={MIN_RADIUS * 100}
                max={MAX_RADIUS * 100}
                step={0.1}
                decimals={1}
                def={2.5}
                unit="%"
                testId="brush-radius"
                onLive={(v) => useRepair.getState().setRadius(clampRadius(v / 100))}
                onCommit={() => undefined}
              />
              <div className="flex items-center gap-1.5">
                <button type="button" className="btn btn-primary" disabled={strokes.length === 0 || inpaintBusy} onClick={applyBrush} data-testid="brush-apply">
                  {inpaintBusy ? <Loader2 size={14} className="animate-spin" /> : <Check size={14} />}
                  {t('repair.apply')}
                  {strokes.length > 0 && <span className="tnum opacity-80">({strokes.length})</span>}
                </button>
                <button type="button" className="btn btn-icon" disabled={strokes.length === 0} onClick={() => useRepair.getState().undoStroke()} title={t('repair.undoStroke')} aria-label={t('repair.undoStroke')} data-testid="brush-undo">
                  <Undo2 size={14} />
                </button>
                <button type="button" className="btn btn-icon" disabled={strokes.length === 0} onClick={() => useRepair.getState().clearStrokes()} title={t('repair.clearStrokes')} aria-label={t('repair.clearStrokes')} data-testid="brush-clear">
                  <X size={14} />
                </button>
              </div>
            </>
          )}
        </div>

        {(['face_restore', 'denoise'] as const).map((op) => (
          <div key={op} className="flex flex-col gap-1.5 rounded-control border border-line p-2">
            <Slider
              label={t(op === 'face_restore' ? 'repair.faceRestore' : 'repair.denoise')}
              value={Math.round(strength[op] * 100)}
              min={0}
              max={100}
              step={1}
              def={op === 'face_restore' ? 70 : 60}
              unit="%"
              testId={`enhance-${op}-strength`}
              onLive={(v) => setStrength((s) => ({ ...s, [op]: v / 100 }))}
              onCommit={() => undefined}
            />
            <button type="button" className="btn w-full justify-start" disabled={enhanceBusy || strength[op] <= 0} onClick={() => enhance(op)} data-testid={`enhance-${op}`}>
              {enhanceBusy ? <Loader2 size={14} className="animate-spin" /> : op === 'face_restore' ? <Sparkles size={14} /> : <Wand2 size={14} />}
              {t(op === 'face_restore' ? 'repair.faceRestoreRun' : 'repair.denoiseRun')}
            </button>
          </div>
        ))}

        {burstOk && (
          <button
            type="button"
            className="btn w-full justify-start text-ai"
            onClick={() => navigate(`/s/${photo.session_id}/besttake/${photo.burst_id}?base=${photo.id}`)}
            data-testid="repair-besttake"
          >
            <SmilePlus size={14} />
            {t('repair.bestTake')}
          </button>
        )}

        <div data-testid="patch-layers">
          <div className="mb-1 text-xs font-medium text-muted">{t('repair.layers', { n: layers.length })}</div>
          {layers.length === 0 ? (
            <div className="text-xs text-faint">{t('repair.noLayers')}</div>
          ) : (
            <ul className="flex flex-col gap-1">
              {layers.map(({ op, index }) => {
                const on = isPatchEnabled(op)
                const name = layerName(op.kind, op.person_id)
                return (
                  <li key={`${op.asset}-${index}`} className="flex items-center gap-2 rounded-control border border-line px-1.5 py-1" data-testid="patch-layer" data-kind={op.kind} data-enabled={on}>
                    <span className="h-7 w-7 shrink-0 overflow-hidden rounded-sm bg-[repeating-conic-gradient(#8884_0_25%,transparent_0_50%)] [background-size:8px_8px]">
                      <img src={assetUrl(photo.id, op.asset)} alt="" className="h-full w-full object-cover" style={{ opacity: on ? 1 : 0.35 }} draggable={false} />
                    </span>
                    <span className={`min-w-0 flex-1 truncate text-xs ${on ? '' : 'text-faint line-through'}`} title={name}>
                      {name}
                    </span>
                    <button
                      type="button"
                      className="btn btn-ghost btn-icon !h-6 !w-6"
                      aria-pressed={on}
                      aria-label={t(on ? 'repair.hideLayer' : 'repair.showLayer')}
                      title={t(on ? 'repair.hideLayer' : 'repair.showLayer')}
                      onClick={() => changeStack(qc, setPatchEnabled(useEdit.getState().stack, index, !on), t(on ? 'history.layerHide' : 'history.layerShow', { name }))}
                      data-testid="patch-toggle"
                    >
                      {on ? <Eye size={13} /> : <EyeOff size={13} />}
                    </button>
                    <button
                      type="button"
                      className="btn btn-ghost btn-icon !h-6 !w-6"
                      aria-label={t('repair.deleteLayer')}
                      title={t('repair.deleteLayer')}
                      onClick={() => changeStack(qc, removePatch(useEdit.getState().stack, index), t('history.layerDelete', { name }))}
                      data-testid="patch-delete"
                    >
                      <Trash2 size={13} />
                    </button>
                  </li>
                )
              })}
            </ul>
          )}
        </div>
      </div>
    </Section>
  )
}
