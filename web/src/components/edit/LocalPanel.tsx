import { useQueryClient } from '@tanstack/react-query'
import { ChevronDown, Circle, Eye, Loader2, Minus, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { fetchMask } from '@/api/client'
import { usePhotoAnalysis } from '@/api/queries'
import type { AdjustKey, MaskRef, MaskTarget, Photo } from '@/api/types'
import { missingModelsOf } from '@/lib/analysis'
import { qk } from '@/lib/cache'
import { addLocal, defaultMask, localOps, maskLabelKey, removeLocal, setLocalField, updateLocal } from '@/lib/edit'
import { changeStack, commitLive, setLive } from '@/lib/editActions'
import { useAnalysisUi } from '@/stores/analysis'
import { useEdit } from '@/stores/edit'
import { useToasts } from '@/stores/toasts'
import { AdjustSliders } from './AdjustSliders'
import { Section } from './Section'
import { Slider } from './Slider'

const AI_CHIPS: MaskTarget[] = ['subject', 'sky', 'background', 'person', 'skin', 'hair', 'clothes']
const LOCAL_KEYS: AdjustKey[] = ['exposure', 'contrast', 'highlights', 'shadows', 'temp', 'saturation', 'clarity', 'dehaze']

/** Local adjustments: AI masks (target chips), radial / linear gradients, per-mask sliders. */
export function LocalPanel({ photo }: { photo: Photo }) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const stack = useEdit((s) => s.stack)
  const active = useEdit((s) => s.activeLocal)
  const overlay = useEdit((s) => s.maskOverlay)
  const flash = useEdit((s) => s.flash)
  const [busy, setBusy] = useState<string | null>(null)
  const [personMenu, setPersonMenu] = useState(false)
  const analysis = usePhotoAnalysis(photo.id)
  const people = new Map<number, string>()
  for (const f of analysis.data?.photo_id === photo.id ? analysis.data.faces : [])
    if (f.person_id !== null) people.set(f.person_id, f.person_name ?? `#${f.person_id}`)

  const locals = localOps(stack)

  const commitAdd = (mask: MaskRef, label: string) => {
    const next = addLocal(useEdit.getState().stack, mask)
    changeStack(qc, next, t('history.addLocal', { name: label }))
    useEdit.getState().setActiveLocal(localOps(next).length - 1)
  }

  /** AI target: fetch the mask first (409 -> consent dialog, then retry), then add the local op and show the overlay. */
  const addAi = async (target: MaskTarget, personId?: number) => {
    const id = photo.id
    setBusy(target)
    try {
      await qc.fetchQuery({ queryKey: qk.mask(id, target, personId), queryFn: () => fetchMask(id, target, personId), staleTime: Infinity })
    } catch (err) {
      const missing = missingModelsOf(err)
      if (missing) {
        useAnalysisUi.getState().setConsent({ sessionId: photo.session_id, profile: 'fast', models: missing, onReady: () => void addAi(target, personId) })
      } else useToasts.getState().push('error', err instanceof Error ? err.message : String(err), 5000)
      return
    } finally {
      setBusy(null)
    }
    if (useEdit.getState().photoId !== id) return
    commitAdd({ kind: 'ai', target, ...(personId !== undefined ? { person_id: personId } : {}) }, t(`edit.target_${target}`))
    useEdit.getState().setMaskOverlay(true)
  }

  return (
    <Section id="local" title={t('edit.local')}>
      <div className="flex flex-wrap gap-1.5" data-testid="local-targets">
        {AI_CHIPS.map((target) =>
          target === 'person' ? (
            <div key={target} className="relative">
              <button className="chip chip-ai" onClick={() => setPersonMenu((o) => !o)} aria-expanded={personMenu} data-testid="target-person">
                {busy === 'person' ? <Loader2 size={11} className="animate-spin" /> : null}
                {t('edit.target_person')}
                <ChevronDown size={11} />
              </button>
              {personMenu && (
                <div className="anim-pop absolute top-7 left-0 z-20 min-w-32 rounded-card border border-line bg-elevated p-1 shadow-[var(--shadow)]">
                  <button
                    className="block w-full rounded px-2 py-1 text-left text-xs hover:bg-panel"
                    onClick={() => {
                      setPersonMenu(false)
                      void addAi('person')
                    }}
                  >
                    {t('edit.anyPerson')}
                  </button>
                  {[...people].map(([pid, name]) => (
                    <button
                      key={pid}
                      className="block w-full rounded px-2 py-1 text-left text-xs hover:bg-panel"
                      onClick={() => {
                        setPersonMenu(false)
                        void addAi('person', pid)
                      }}
                    >
                      {name}
                    </button>
                  ))}
                </div>
              )}
            </div>
          ) : (
            <button key={target} className="chip chip-ai" disabled={busy !== null} onClick={() => void addAi(target)} data-testid={`target-${target}`}>
              {busy === target ? <Loader2 size={11} className="animate-spin" /> : null}
              {t(`edit.target_${target}`)}
            </button>
          ),
        )}
      </div>
      <div className="mt-1.5 flex gap-1.5">
        <button className="chip" onClick={() => commitAdd(defaultMask('radial'), t('edit.mask_radial'))} data-testid="add-radial">
          <Circle size={11} />
          {t('edit.mask_radial')}
        </button>
        <button className="chip" onClick={() => commitAdd(defaultMask('linear'), t('edit.mask_linear'))} data-testid="add-linear">
          <Minus size={11} />
          {t('edit.mask_linear')}
        </button>
        <button className="chip ml-auto" aria-pressed={overlay} onClick={() => useEdit.getState().setMaskOverlay(!overlay)} title={`${t('edit.maskOverlay')} (O)`} data-testid="mask-overlay-toggle">
          <Eye size={11} />
          O
        </button>
      </div>

      {locals.length === 0 ? (
        <p className="mt-2 text-[11px] text-faint">{t('edit.localEmpty')}</p>
      ) : (
        <ul className="mt-2 flex flex-col gap-1.5" data-testid="local-list">
          {locals.map(({ op, index }) => {
            const open = active === index
            const name = op.mask.kind === 'ai' && op.mask.person_id !== undefined ? `${t('edit.target_person')} ${people.get(op.mask.person_id) ?? `#${op.mask.person_id}`}` : t(maskLabelKey(op.mask))
            return (
              <li key={index} className={`rounded-card border ${open ? 'border-accent/60 bg-bg/40' : 'border-line'}`} data-testid="local-item">
                <div className="flex items-center gap-1 px-2 py-1">
                  <button className="flex-1 truncate text-left text-xs font-medium" onClick={() => useEdit.getState().setActiveLocal(open ? null : index)}>
                    <span className={op.mask.kind === 'ai' ? 'text-ai' : ''}>{name}</span>
                  </button>
                  <button
                    className="chip !h-5 px-1.5 text-[11px]"
                    aria-pressed={!!op.invert}
                    onClick={() => changeStack(qc, updateLocal(useEdit.getState().stack, index, (o) => ({ ...o, invert: !o.invert })), t('history.invertMask'))}
                  >
                    {t('edit.invert')}
                  </button>
                  <button
                    className="btn btn-ghost btn-icon !h-6 !w-6"
                    aria-label={t('edit.removeLocal')}
                    title={t('edit.removeLocal')}
                    data-testid="remove-local"
                    onClick={() => {
                      changeStack(qc, removeLocal(useEdit.getState().stack, index), t('history.removeLocal', { name }))
                      useEdit.getState().setActiveLocal(null)
                    }}
                  >
                    <Trash2 size={12} />
                  </button>
                </div>
                {open && (
                  <div className="border-t border-line/60 px-2 py-1.5">
                    <Slider
                      label={t('edit.amount')}
                      value={Math.round((op.amount ?? 1) * 100)}
                      min={0}
                      max={100}
                      step={1}
                      def={100}
                      unit="%"
                      onLive={(v) => setLive(updateLocal(useEdit.getState().stack, index, (o) => ({ ...o, amount: v / 100 })))}
                      onCommit={() => commitLive(qc, t('history.localAmount'), `local:${index}:amount`)}
                    />
                    <AdjustSliders
                      prefix={`local${index}`}
                      keys={LOCAL_KEYS}
                      adjust={op.adjust}
                      flash={flash}
                      onSet={(k, v) => setLive(setLocalField(useEdit.getState().stack, index, k, v))}
                      onCommit={(k) => commitLive(qc, t('history.localAdjust', { name: t(`adjust.${k}`) }), `local:${index}:${k}`)}
                    />
                  </div>
                )}
              </li>
            )
          })}
        </ul>
      )}
    </Section>
  )
}
