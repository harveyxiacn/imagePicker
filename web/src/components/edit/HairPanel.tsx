import { useQueryClient } from '@tanstack/react-query'
import { AlertTriangle, Check, Droplets, Eye, EyeOff, Loader2, RefreshCw, Scissors, Trash2, Wind } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { api, faceCropUrl, thumbUrlById } from '@/api/client'
import { useHairDonors, usePhotoPeople } from '@/api/queries'
import type { HairDonor, Photo, SweatOp } from '@/api/types'
import { setLive, changeStack, commitLive } from '@/lib/editActions'
import { runGen } from '@/lib/gen'
import { bestDonor, defaultHairFace, donorReasonKeys, hairLayerOf, removeHair, removeSweat, scorePct, setHairEnabled, setHairStrength, setSweatStrength, sweatLayerOf, timeOffset } from '@/lib/hair'
import { useEdit } from '@/stores/edit'
import { useGen } from '@/stores/gen'
import { GenTasks } from '../GenTasks'
import { Section } from './Section'
import { Slider } from './Slider'

/** One donor in the strip: thumbnail + donor face, score, and why (or why not). */
function DonorCard({ d, chosen, onPick }: { d: HairDonor; chosen: boolean; onPick: () => void }) {
  const { t } = useTranslation()
  const pct = scorePct(d.score)
  const why = donorReasonKeys(d).map((k) => t(k))
  const off = timeOffset(d.minutes_from_target)
  const when = off ? t(off.key, { h: off.h, m: off.m }) : null
  const label = [d.file_name, when, pct !== null ? t('hair.score', { n: pct }) : null, ...why].filter(Boolean).join(' · ')
  return (
    <button
      type="button"
      role="option"
      aria-selected={chosen}
      aria-disabled={!d.composable || undefined}
      aria-label={label}
      title={label}
      disabled={!d.composable}
      onClick={onPick}
      className="relative flex w-[84px] shrink-0 flex-col gap-1 rounded-control border p-1 text-left disabled:opacity-60"
      style={{ borderColor: chosen ? 'var(--accent)' : 'var(--line)' }}
      data-testid="hair-donor"
      data-composable={d.composable}
    >
      <span className="relative block h-[56px] w-full overflow-hidden rounded-sm bg-bg">
        <img src={thumbUrlById(d.photo_id)} alt="" className="h-full w-full object-cover" draggable={false} loading="lazy" />
        <img
          src={faceCropUrl(d.face_id, 128)}
          alt=""
          className="absolute right-0.5 bottom-0.5 h-6 w-6 rounded-full border border-bg object-cover"
          draggable={false}
          loading="lazy"
        />
      </span>
      <span className="flex items-center gap-1 text-[11px] leading-tight">
        {d.composable ? (
          pct !== null && <span className="tnum font-semibold">{pct}</span>
        ) : (
          <AlertTriangle size={10} className="shrink-0 text-warning" aria-hidden />
        )}
        <span className="truncate text-muted">{why[0] ?? when ?? ''}</span>
      </span>
    </button>
  )
}

/** One sweat cleanup op (去汗光 / 整理碎发): run it, then its layer's strength / remove. No donor photo needed. */
function SweatRow({ photo, op, busy }: { photo: Photo; op: SweatOp; busy: boolean }) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const stack = useEdit((s) => s.stack)
  const mine = useEdit((s) => s.photoId === photo.id)
  const layer = mine ? sweatLayerOf(stack, op) : undefined
  const [strength, setStrength] = useState(op === 'deshine' ? 70 : 100)
  const cur = () => useEdit.getState().stack
  const run = () =>
    void runGen(qc, {
      kind: 'enhance',
      sessionId: photo.session_id,
      photoId: photo.id,
      label: t(`history.${op}`),
      start: () => api.enhance(photo.id, { op, strength: strength / 100 }),
    })
  return (
    <div className="flex flex-col gap-1 rounded-control border border-line p-1.5" data-testid={`sweat-${op}`}>
      <Slider
        label={t(`hair.${op}`)}
        value={layer ? Math.round((layer.amount ?? 1) * 100) : strength}
        min={0}
        max={100}
        step={1}
        def={op === 'deshine' ? 70 : 100}
        unit="%"
        testId={`sweat-${op}-strength`}
        onLive={(v) => (layer ? setLive(setSweatStrength(cur(), op, v / 100)) : setStrength(v))}
        onCommit={() => {
          if (layer) commitLive(qc, t('history.sweatStrength', { name: t(`hair.${op}`) }), `sweat:${op}`)
        }}
      />
      <div className="flex items-center gap-1">
        <button type="button" className="btn flex-1 justify-start" disabled={busy || (!layer && strength <= 0)} onClick={run} data-testid={`sweat-${op}-run`}>
          {busy ? <Loader2 size={14} className="animate-spin" /> : op === 'deshine' ? <Droplets size={14} /> : <Wind size={14} />}
          {t(layer ? 'hair.sweatRedo' : `hair.${op}Run`)}
        </button>
        {layer && (
          <button
            type="button"
            className="btn btn-ghost btn-icon !h-7 !w-7"
            aria-label={t('hair.sweatRemove', { name: t(`hair.${op}`) })}
            title={t('hair.sweatRemove', { name: t(`hair.${op}`) })}
            onClick={() => changeStack(qc, removeSweat(cur(), op), t('history.sweatRemove', { name: t(`hair.${op}`) }))}
            data-testid={`sweat-${op}-remove`}
          >
            <Trash2 size={13} />
          </button>
        )}
      </div>
    </div>
  )
}

/**
 * 发型 section of the edit panel (docs/04 section 3.5, docs/api-contract-m5.md H): replace a person's hair with their
 * own hair from another photo (same person, similar pose). Pick face -> donor strip -> apply -> strength / show / undo.
 */
export function HairPanel({ photo }: { photo: Photo }) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const stack = useEdit((s) => s.stack)
  const stackOfThisPhoto = useEdit((s) => s.photoId === photo.id)
  const tasks = useGen((s) => s.tasks)
  const peopleQ = usePhotoPeople(photo.id)
  const people = useMemo(() => peopleQ.data?.people ?? [], [peopleQ.data])
  const [chosenFace, setChosenFace] = useState<number | undefined>(undefined)
  const faceId = chosenFace !== undefined && people.some((p) => p.face_id === chosenFace) ? chosenFace : defaultHairFace(people)
  const [look, setLook] = useState(false)
  const donorsQ = useHairDonors(photo.id, faceId, look)
  const donors = donorsQ.data?.candidates ?? []
  const [picked, setPicked] = useState<number | undefined>(undefined)
  const donor = donors.find((d) => d.face_id === picked && d.composable) ?? bestDonor(donors)
  const [strength, setStrength] = useState(100)
  const busy = Object.values(tasks).some((x) => x.kind === 'hair' && x.photoId === photo.id && x.state === 'running')
  const enhanceBusy = Object.values(tasks).some((x) => x.kind === 'enhance' && x.photoId === photo.id && x.state === 'running')
  const layer = stackOfThisPhoto && faceId !== undefined ? hairLayerOf(stack, faceId) : undefined
  const layerOn = layer?.enabled !== false
  const cur = () => useEdit.getState().stack
  const who = (id: number | undefined) => {
    const p = people.find((x) => x.face_id === id)
    return p?.person_name ?? (p?.person_id != null ? `#${p.person_id}` : t('hair.thisFace'))
  }

  const apply = () => {
    if (!donor || faceId === undefined) return
    void runGen(qc, {
      kind: 'hair',
      sessionId: photo.session_id,
      photoId: photo.id,
      label: t('history.hair', { who: who(faceId) }),
      start: () => api.hair(photo.id, { face_id: faceId, donor_photo_id: donor.photo_id, donor_face_id: donor.face_id, strength: strength / 100 }),
    })
  }

  return (
    <Section id="hair" icon={<Scissors size={13} />} title={t('hair.title')} defaultCollapsed>
      <div className="flex flex-col gap-2" data-testid="hair-panel">
        <p className="text-[11px] text-muted">{t('hair.intro')}</p>
        {peopleQ.isPending ? (
          <div className="flex items-center gap-2 py-1 text-xs text-muted">
            <Loader2 size={13} className="animate-spin" />
            {t('library.loading')}
          </div>
        ) : people.length === 0 ? (
          <p className="text-[11px] text-faint" data-testid="hair-none">
            {t('hair.noFaces')}
          </p>
        ) : (
          <>
            {people.length > 1 && (
              <div className="flex flex-wrap gap-1.5" role="radiogroup" aria-label={t('hair.face')} data-testid="hair-faces">
                {people.map((p) => (
                  <button
                    key={p.face_id}
                    type="button"
                    role="radio"
                    aria-checked={p.face_id === faceId}
                    className="flex items-center gap-1 rounded-full border py-0.5 pr-2 pl-0.5 text-xs"
                    style={{ borderColor: p.face_id === faceId ? 'var(--accent)' : 'var(--line)' }}
                    onClick={() => {
                      setChosenFace(p.face_id)
                      setPicked(undefined)
                    }}
                    data-testid="hair-face"
                  >
                    <img src={faceCropUrl(p.face_id, 128)} alt="" className="h-6 w-6 rounded-full object-cover" draggable={false} />
                    <span className="max-w-16 truncate">{who(p.face_id)}</span>
                  </button>
                ))}
              </div>
            )}

            {!look ? (
              <button type="button" className="btn btn-ai w-full justify-start" disabled={faceId === undefined} onClick={() => setLook(true)} data-testid="hair-find">
                <Scissors size={14} />
                {t('hair.find')}
              </button>
            ) : donorsQ.isPending ? (
              <div className="flex items-center gap-2 py-1 text-xs text-muted" role="status">
                <Loader2 size={13} className="animate-spin" />
                {t('hair.ranking')}
              </div>
            ) : donorsQ.isError ? (
              <div className="text-xs text-danger">{(donorsQ.error as Error).message}</div>
            ) : donors.length === 0 ? (
              <p className="text-[11px] text-faint" data-testid="hair-no-donors">
                {t('hair.noDonors')}
              </p>
            ) : (
              <>
                {donorsQ.data && !donorsQ.data.ranked && (
                  <p className="text-[11px] text-warning" data-testid="hair-unranked">
                    {t(donorsQ.data.rank_error === 'models_missing' ? 'hair.unrankedModels' : 'hair.unranked')}
                  </p>
                )}
                <div className="flex items-center justify-between text-[11px] text-muted">
                  <span id="hair-donors-label">{t('hair.donors', { n: donors.filter((d) => d.composable).length })}</span>
                  <button
                    type="button"
                    className="btn btn-ghost btn-icon !h-6 !w-6"
                    title={t('hair.refresh')}
                    aria-label={t('hair.refresh')}
                    onClick={() => void donorsQ.refetch()}
                  >
                    <RefreshCw size={12} />
                  </button>
                </div>
                <div className="flex gap-1.5 overflow-x-auto pb-1" role="listbox" aria-labelledby="hair-donors-label" data-testid="hair-donors">
                  {donors.map((d) => (
                    <DonorCard key={d.face_id} d={d} chosen={donor?.face_id === d.face_id} onPick={() => setPicked(d.face_id)} />
                  ))}
                </div>
                {donor && (
                  <div className="flex items-center gap-2 rounded-control border border-line p-1.5" data-testid="hair-preview">
                    <figure className="flex flex-col items-center gap-0.5">
                      <img src={faceCropUrl(faceId!, 256)} alt={t('hair.now')} className="h-14 w-14 rounded-sm object-cover" draggable={false} />
                      <figcaption className="text-[11px] text-muted">{t('hair.now')}</figcaption>
                    </figure>
                    <span aria-hidden className="text-muted">→</span>
                    <figure className="flex flex-col items-center gap-0.5">
                      <img src={faceCropUrl(donor.face_id, 256)} alt={t('hair.from', { name: donor.file_name })} className="h-14 w-14 rounded-sm object-cover" draggable={false} />
                      <figcaption className="max-w-20 truncate text-[11px] text-muted">{donor.file_name}</figcaption>
                    </figure>
                    <ul className="min-w-0 flex-1 text-[11px] leading-snug text-muted">
                      {donorReasonKeys(donor).map((k) => (
                        <li key={k} className="truncate">
                          {t(k)}
                        </li>
                      ))}
                    </ul>
                  </div>
                )}
                <Slider
                  label={t('hair.strength')}
                  value={strength}
                  min={0}
                  max={100}
                  step={1}
                  def={100}
                  unit="%"
                  testId="hair-strength-new"
                  onLive={setStrength}
                  onCommit={() => undefined}
                />
                <button type="button" className="btn btn-primary w-full justify-start" disabled={!donor || busy || strength <= 0} onClick={apply} data-testid="hair-apply">
                  {busy ? <Loader2 size={14} className="animate-spin" /> : <Check size={14} />}
                  {layer ? t('hair.replace') : t('hair.apply')}
                </button>
                <GenTasks photoId={photo.id} className="flex-wrap text-xs" />
              </>
            )}

            {layer && faceId !== undefined && (
              <div className="flex flex-col gap-1 rounded-control border border-line p-1.5" data-testid="hair-layer">
                <div className="flex items-center gap-1 text-[11px]">
                  <span className="min-w-0 flex-1 truncate">{t('hair.applied', { who: who(faceId) })}</span>
                  <button
                    type="button"
                    className="btn btn-ghost btn-icon !h-6 !w-6"
                    aria-pressed={layerOn}
                    aria-label={t(layerOn ? 'hair.hide' : 'hair.show')}
                    title={t(layerOn ? 'hair.hide' : 'hair.show')}
                    onClick={() => changeStack(qc, setHairEnabled(cur(), faceId, !layerOn), t(layerOn ? 'history.hairHide' : 'history.hairShow', { who: who(faceId) }))}
                    data-testid="hair-toggle"
                  >
                    {layerOn ? <Eye size={13} /> : <EyeOff size={13} />}
                  </button>
                  <button
                    type="button"
                    className="btn btn-ghost btn-icon !h-6 !w-6"
                    aria-label={t('hair.remove')}
                    title={t('hair.remove')}
                    onClick={() => changeStack(qc, removeHair(cur(), faceId), t('history.hairRemove', { who: who(faceId) }))}
                    data-testid="hair-remove"
                  >
                    <Trash2 size={13} />
                  </button>
                </div>
                <Slider
                  label={t('hair.strength')}
                  value={Math.round((layer.amount ?? 1) * 100)}
                  min={0}
                  max={100}
                  step={1}
                  def={100}
                  unit="%"
                  testId="hair-strength"
                  disabled={!layerOn}
                  onLive={(v) => setLive(setHairStrength(cur(), faceId, v / 100))}
                  onCommit={() => commitLive(qc, t('history.hairStrength', { who: who(faceId) }), `hair:${faceId}`)}
                />
                {(layer.warnings?.length ?? 0) > 0 && (
                  <p className="text-[11px] text-warning">{layer.warnings!.map((w) => t(`hair.warn_${w}`, { defaultValue: w })).join(' · ')}</p>
                )}
              </div>
            )}
            <div className="flex flex-col gap-1.5 border-t border-line/60 pt-2" data-testid="sweat-cleanup">
              <div className="text-[11px] font-medium text-muted">{t('hair.sweatTitle')}</div>
              <p className="text-[11px] text-faint">{t('hair.sweatHint')}</p>
              {(['deshine', 'flyaway'] as const).map((op) => (
                <SweatRow key={op} photo={photo} op={op} busy={enhanceBusy} />
              ))}
            </div>
            <p className="text-[11px] text-faint">{t('hair.limits')}</p>
          </>
        )}
      </div>
    </Section>
  )
}
