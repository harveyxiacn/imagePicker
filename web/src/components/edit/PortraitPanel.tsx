import { useQueryClient } from '@tanstack/react-query'
import { AlertTriangle, BookmarkCheck, BookmarkPlus, Loader2, RotateCcw, ScanFace, Star } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { api, ApiError, faceCropUrl } from '@/api/client'
import { usePhotoPeople } from '@/api/queries'
import { LEVELS, type BeautyKey, type Level, type Photo, type PhotoPerson, type WarpBodyKey, type WarpFaceKey } from '@/api/types'
import { missingModelsOf } from '@/lib/analysis'
import {
  applyProfile,
  BEAUTY_SLIDERS,
  BODY_SLIDERS,
  FACE_SLIDERS,
  getBeauty,
  getLevel,
  getWarpBody,
  getWarpFace,
  hasPortrait,
  isUnnatural,
  profileFromStack,
  removePerson,
  setBeautyField,
  setBlemish,
  setBodyField,
  setFaceField,
  setLevel,
  setProtectBackground,
  type PersonKey,
} from '@/lib/beauty'
import { qk } from '@/lib/cache'
import { changeStack, commitLive, setLive } from '@/lib/editActions'
import { useAnalysisUi } from '@/stores/analysis'
import { useEdit } from '@/stores/edit'
import { useToasts } from '@/stores/toasts'
import { Section } from './Section'
import { Slider } from './Slider'

/** One person chip (cover crop + name) or the "everyone" chip. */
function Chip({ active, onClick, edited, children, testId, title }: { active: boolean; onClick: () => void; edited?: boolean; children: React.ReactNode; testId?: string; title?: string }) {
  return (
    <button
      type="button"
      className="relative flex min-w-0 items-center gap-1.5 rounded-full border py-0.5 pr-2.5 pl-0.5 text-xs transition-colors hover:bg-elevated"
      style={{ borderColor: active ? 'var(--accent)' : 'var(--line)', background: active ? 'color-mix(in srgb, var(--accent) 14%, transparent)' : undefined }}
      aria-pressed={active}
      title={title}
      onClick={onClick}
      data-testid={testId}
    >
      {children}
      {edited && <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-accent" aria-hidden />}
    </button>
  )
}

/** Portrait beauty section of the edit panel (doc 04 3.5): per-person beauty / face / body ops. */
export function PortraitPanel({ photo }: { photo: Photo }) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const stack = useEdit((s) => s.stack)
  const [preparing, setPreparing] = useState(false)
  const [chosen, setChosen] = useState<PersonKey | undefined>(undefined)
  const [fallbackLevel, setFallbackLevel] = useState<Record<string, Level>>({})
  const peopleQ = usePhotoPeople(photo.id, preparing)
  const data = peopleQ.data
  const ready = data?.ready ?? false
  const push = useToasts((s) => s.push)

  const chips = (data?.people ?? []).filter((p): p is PhotoPerson & { person_id: number } => p.person_id !== null)
  const defaultTarget: PersonKey = chips.find((c) => c.is_subject)?.person_id ?? null
  const person: PersonKey = chosen !== undefined && (chosen === null || chips.some((c) => c.person_id === chosen)) ? chosen : defaultTarget
  const personInfo = person === null ? null : (chips.find((c) => c.person_id === person) ?? null)
  const hasPose = person === null ? (data?.people ?? []).some((p) => p.has_pose) : !!personInfo?.has_pose
  const level = getLevel(stack, person, fallbackLevel[String(person)])
  const beauty = getBeauty(stack, person)
  const face = getWarpFace(stack, person)
  const body = getWarpBody(stack, person)
  const edited = hasPortrait(stack, person)

  const prepare = async () => {
    setPreparing(true)
    try {
      await api.beautyPrepare(photo.id)
    } catch (err) {
      setPreparing(false)
      const missing = missingModelsOf(err)
      if (missing) {
        // 409 models_missing: the consent dialog downloads the models, then we try again.
        useAnalysisUi.getState().setConsent({ sessionId: photo.session_id, profile: 'fast', models: missing, onReady: () => void prepare() })
      } else push('error', err instanceof ApiError || err instanceof Error ? err.message : String(err), 5000)
    }
  }
  const preparingNow = preparing && !ready

  const cur = () => useEdit.getState().stack
  const lv = () => getLevel(cur(), person, fallbackLevel[String(person)])
  const who = person === null ? t('beauty.everyone') : (personInfo?.person_name ?? `#${person}`)
  const label = (name: string) => t('history.portrait', { name, who })

  const setLevelTo = (l: Level) => {
    setFallbackLevel((m) => ({ ...m, [String(person)]: l }))
    if (hasPortrait(cur(), person) || getBeauty(cur(), person) || getWarpFace(cur(), person) || getWarpBody(cur(), person))
      changeStack(qc, setLevel(cur(), person, l), t('history.portraitLevel', { level: t(`beauty.level_${l}`), who }))
  }

  const saveProfile = async () => {
    if (person === null) return
    const profile = profileFromStack(cur(), person)
    if (!profile) {
      push('info', t('beauty.profileEmpty'), 2500)
      return
    }
    try {
      await api.putBeautyProfile(person, profile)
      void qc.invalidateQueries({ queryKey: qk.photoPeopleAll })
      push('success', t('beauty.profileSaved', { who }), 2500)
    } catch (err) {
      push('error', err instanceof Error ? err.message : String(err), 5000)
    }
  }
  const applySaved = async () => {
    if (person === null) return
    try {
      const { profile } = await api.beautyProfile(person)
      if (!profile) {
        push('info', t('beauty.profileNone'), 2500)
        return
      }
      changeStack(qc, applyProfile(cur(), profile, person), t('history.applyProfile', { who }))
    } catch (err) {
      push('error', err instanceof Error ? err.message : String(err), 5000)
    }
  }

  return (
    <Section
      id="portrait"
      icon={<ScanFace size={13} />}
      title={t('beauty.title')}
      right={
        ready && (
          <button
            className="btn btn-ghost btn-icon !h-6 !w-6"
            disabled={!edited}
            title={t('edit.resetSection')}
            aria-label={t('edit.resetSection')}
            data-testid="portrait-reset"
            onClick={() => changeStack(qc, removePerson(cur(), person), t('history.portraitReset', { who }))}
          >
            <RotateCcw size={12} />
          </button>
        )
      }
    >
      {peopleQ.isPending ? (
        <div className="flex items-center gap-2 py-2 text-xs text-muted">
          <Loader2 size={13} className="animate-spin" />
          {t('library.loading')}
        </div>
      ) : peopleQ.isError ? (
        <div className="text-xs text-danger">{(peopleQ.error as Error).message}</div>
      ) : (data?.people.length ?? 0) === 0 ? (
        <p className="text-[11px] text-faint" data-testid="portrait-none">
          {t('beauty.noPeople')}
        </p>
      ) : !ready ? (
        <div className="flex flex-col items-start gap-2" data-testid="portrait-prepare">
          <p className="text-[11px] text-muted">{t('beauty.prepareHint', { n: data?.people.length ?? 0 })}</p>
          <button className="btn btn-ai" disabled={preparingNow} onClick={() => void prepare()} data-testid="beauty-prepare">
            {preparingNow ? <Loader2 size={13} className="animate-spin" /> : <ScanFace size={13} />}
            {preparingNow ? t('beauty.preparing') : t('beauty.prepare')}
          </button>
        </div>
      ) : (
        <div className="flex flex-col gap-2" data-testid="portrait-ready">
          <div>
            <div className="mb-1 text-[11px] text-muted">{t('beauty.appliesTo')}</div>
            <div className="flex flex-wrap gap-1.5" role="group" aria-label={t('beauty.appliesTo')} data-testid="portrait-chips">
              <Chip active={person === null} onClick={() => setChosen(null)} edited={hasPortrait(stack, null)} testId="chip-everyone">
                <span className="flex h-6 w-6 items-center justify-center rounded-full bg-bg text-muted">
                  <ScanFace size={13} />
                </span>
                {t('beauty.everyone')}
              </Chip>
              {chips.map((c) => (
                <Chip
                  key={c.face_id}
                  active={person === c.person_id}
                  onClick={() => setChosen(c.person_id)}
                  edited={hasPortrait(stack, c.person_id)}
                  testId="chip-person"
                  title={c.has_profile ? t('beauty.hasProfile') : undefined}
                >
                  <img src={faceCropUrl(c.face_id, 128)} alt="" className="h-6 w-6 shrink-0 rounded-full object-cover" draggable={false} />
                  <span className="max-w-16 truncate">{c.person_name ?? `${t('person.unnamed')} ${c.person_id}`}</span>
                  {c.has_profile && <Star size={10} className="shrink-0 text-accent" fill="currentColor" />}
                </Chip>
              ))}
            </div>
          </div>

          <div className="flex items-center gap-2">
            <span className="text-[11px] text-muted">{t('beauty.level')}</span>
            <div className="flex flex-1 rounded-control border border-line p-0.5" role="radiogroup" aria-label={t('beauty.level')} data-testid="level-control">
              {LEVELS.map((l) => (
                <button
                  key={l}
                  role="radio"
                  aria-checked={level === l}
                  className={`h-6 flex-1 rounded text-xs transition-colors ${level === l ? 'bg-accent text-accent-fg' : 'text-muted hover:text-fg'}`}
                  onClick={() => setLevelTo(l)}
                  data-testid={`level-${l}`}
                >
                  {t(`beauty.level_${l}`)}
                </button>
              ))}
            </div>
          </div>

          {isUnnatural(stack, person) && (
            <div className="flex items-start gap-1.5 rounded-card border border-warning/50 bg-warning/10 px-2 py-1.5 text-[11px] text-warning" role="status" data-testid="unnatural-warning">
              <AlertTriangle size={13} className="mt-px shrink-0" />
              {t('beauty.unnatural')}
            </div>
          )}

          <div className="border-t border-line/60 pt-1" data-testid="beauty-sliders">
            <div className="mb-0.5 text-[11px] font-medium text-muted">{t('beauty.skin')}</div>
            {BEAUTY_SLIDERS.map(({ key, min, max }) => (
              <Slider
                key={key}
                testId={`beauty-${key}`}
                label={t(`beauty.${key}`)}
                value={beauty?.[key] ?? 0}
                min={min}
                max={max}
                step={1}
                onLive={(v) => setLive(setBeautyField(cur(), person, key as BeautyKey, v, lv()))}
                onCommit={() => commitLive(qc, label(t(`beauty.${key}`)), `beauty:${person}:${key}`)}
              />
            ))}
            <div className="flex items-center gap-2 py-[3px]">
              <span className="w-[72px] shrink-0 truncate text-[12px] text-muted">{t('beauty.blemish')}</span>
              <button
                className="chip"
                aria-pressed={!!beauty?.blemish}
                data-testid="beauty-blemish"
                onClick={() => changeStack(qc, setBlemish(cur(), person, !beauty?.blemish, lv()), label(t('beauty.blemish')))}
              >
                {beauty?.blemish ? t('beauty.on') : t('beauty.off')}
              </button>
            </div>
          </div>

          <div className="border-t border-line/60 pt-1" data-testid="face-sliders">
            <div className="mb-0.5 text-[11px] font-medium text-muted">{t('beauty.faceShape')}</div>
            {FACE_SLIDERS.map(({ key, min, max }) => (
              <Slider
                key={key}
                testId={`face-${key}`}
                label={t(`beauty.${key}`)}
                value={face?.[key] ?? 0}
                min={min}
                max={max}
                step={1}
                onLive={(v) => setLive(setFaceField(cur(), person, key as WarpFaceKey, v, lv()))}
                onCommit={() => commitLive(qc, label(t(`beauty.${key}`)), `face:${person}:${key}`)}
              />
            ))}
          </div>

          <div className="border-t border-line/60 pt-1" data-testid="body-sliders" aria-disabled={!hasPose || undefined}>
            <div className="mb-0.5 text-[11px] font-medium text-muted">{t('beauty.body')}</div>
            {!hasPose && (
              <p className="mb-1 text-[11px] text-faint" data-testid="no-pose-hint">
                {t('beauty.noPose')}
              </p>
            )}
            {BODY_SLIDERS.map(({ key, min, max }) => (
              <Slider
                key={key}
                testId={`body-${key}`}
                label={t(`beauty.${key}`)}
                value={body?.[key] ?? 0}
                min={min}
                max={max}
                step={1}
                disabled={!hasPose}
                onLive={(v) => setLive(setBodyField(cur(), person, key as WarpBodyKey, v, lv()))}
                onCommit={() => commitLive(qc, label(t(`beauty.${key}`)), `body:${person}:${key}`)}
              />
            ))}
            <div className="flex items-center gap-2 py-[3px]">
              <span className="w-[72px] shrink-0 truncate text-[12px] text-muted" title={t('beauty.protectBackground')}>
                {t('beauty.protectBackground')}
              </span>
              <button
                className="chip"
                disabled={!hasPose}
                aria-pressed={body?.protect_background ?? true}
                data-testid="body-protect"
                onClick={() => changeStack(qc, setProtectBackground(cur(), person, !(body?.protect_background ?? true), lv()), label(t('beauty.protectBackground')))}
              >
                {(body?.protect_background ?? true) ? t('beauty.on') : t('beauty.off')}
              </button>
            </div>
          </div>

          <div className="flex flex-wrap gap-1.5 border-t border-line/60 pt-2">
            <button
              className="btn"
              disabled={person === null || !edited}
              title={person === null ? t('beauty.profileNeedsPerson') : undefined}
              onClick={() => void saveProfile()}
              data-testid="profile-save"
            >
              <BookmarkPlus size={13} />
              {t('beauty.saveProfile')}
            </button>
            <button
              className="btn"
              disabled={person === null || !personInfo?.has_profile}
              title={person === null ? t('beauty.profileNeedsPerson') : !personInfo?.has_profile ? t('beauty.profileNone') : undefined}
              onClick={() => void applySaved()}
              data-testid="profile-apply"
            >
              <BookmarkCheck size={13} />
              {t('beauty.applyProfile')}
            </button>
          </div>
        </div>
      )}
    </Section>
  )
}
