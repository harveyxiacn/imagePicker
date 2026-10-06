import { useQueryClient } from '@tanstack/react-query'
import { AlertTriangle, Ban, Check, RotateCcw, Star } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { faceCropUrl } from '@/api/client'
import { sortedWarnings, visibleCandidates, type CandidateView, type PersonView } from '@/lib/besttake'
import { commitLive, setLive } from '@/lib/editActions'
import { bestTakeOf, updateBestTake } from '@/lib/patches'
import { useEdit } from '@/stores/edit'
import { useGen } from '@/stores/gen'
import { Slider } from '../edit/Slider'

interface Props {
  person: PersonView | undefined
  /** another frame was chosen as base: composability is relative to the plan's base, results may differ */
  retargeted: boolean
  busy: boolean
  onChoose: (person: PersonView, c: CandidateView) => void
  onRestore: (person: PersonView) => void
}

/** Right column of the Best Take editor (doc 04 3.4): candidates of the selected person, blend controls, warnings, restore. */
export function BestTakePanel({ person, retargeted, busy, onChoose, onRestore }: Props) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const stack = useEdit((s) => s.stack)
  const result = useGen((s) => (person ? s.results[person.baseFaceId] : undefined))
  const [all, setAll] = useState(false)

  if (!person) {
    return (
      <div className="flex h-full items-center justify-center p-6 text-center text-muted" data-testid="bt-panel-empty">
        {t('besttake.clickFace')}
      </div>
    )
  }
  const shown = visibleCandidates(person, all)
  const patch = person.person_id !== null ? bestTakeOf(stack, person.person_id) : undefined
  const who = person.name ?? t('person.unnamed')
  const warnings = sortedWarnings(result?.ok ? result.warnings : [])
  const frameOfPatch = patch?.source_photo_id !== undefined ? person.candidates.find((c) => c.photo_id === patch.source_photo_id)?.frame : null

  const setBlend = (key: 'amount' | 'feather', v: number) => {
    if (person.person_id === null) return
    setLive(updateBestTake(useEdit.getState().stack, person.person_id, (p) => ({ ...p, [key]: v })))
  }

  return (
    <div className="flex flex-col gap-3 p-3" data-testid="bt-panel">
      <div className="flex items-center gap-2">
        <img src={faceCropUrl(person.baseFaceId, 128)} alt="" className="h-10 w-10 rounded-full object-cover" draggable={false} />
        <div className="min-w-0">
          <div className="truncate font-semibold" data-testid="bt-person-name">
            {who}
          </div>
          <div className="text-xs text-muted">{person.replaced ? t('besttake.statusReplaced', { frame: frameOfPatch ?? '?' }) : t('besttake.statusOriginal')}</div>
        </div>
      </div>

      <div>
        <div className="mb-1.5 text-xs font-medium text-muted">{t('besttake.candidates')}</div>
        <div className="grid grid-cols-3 gap-2" data-testid="bt-candidates">
          {shown.map((c) => {
            const disabled = !c.composable
            return (
              <button
                key={c.photo_id}
                type="button"
                className="relative flex flex-col items-stretch overflow-hidden rounded-control border bg-bg text-left transition-colors enabled:hover:bg-elevated"
                style={{
                  borderColor: c.isCurrent ? 'var(--accent)' : 'var(--line)',
                  boxShadow: c.isCurrent ? '0 0 0 2px var(--accent)' : undefined,
                  opacity: disabled ? 0.45 : 1,
                  cursor: disabled ? 'not-allowed' : 'pointer',
                }}
                aria-disabled={disabled}
                aria-pressed={c.isCurrent}
                disabled={busy && !disabled}
                title={disabled ? `${t('besttake.notComposable')}: ${t(`besttake.reason_${c.reason ?? 'unknown'}`, { defaultValue: c.reason ?? '' })}` : c.isBase ? t('besttake.baseHint') : t('besttake.useHint')}
                onClick={() => (disabled ? undefined : onChoose(person, c))}
                data-testid="bt-candidate"
                data-photo={c.photo_id}
                data-composable={c.composable}
                data-current={c.isCurrent}
              >
                <span className="relative block aspect-square w-full bg-black/20">
                  <img src={faceCropUrl(c.face_id, 128)} alt="" className="h-full w-full object-cover" style={disabled ? { filter: 'grayscale(1)' } : undefined} draggable={false} />
                  {c.isBest && !disabled && <Star size={12} className="absolute top-1 right-1 fill-current text-ai drop-shadow" aria-label={t('besttake.best')} />}
                  {c.isCurrent && (
                    <span className="absolute top-1 left-1 rounded-full bg-accent p-0.5 text-accent-fg">
                      <Check size={10} />
                    </span>
                  )}
                  {disabled && <Ban size={16} className="absolute inset-0 m-auto text-white/80 drop-shadow" />}
                </span>
                <span className="flex items-center justify-between px-1.5 py-1 text-xs">
                  <span className="tnum font-medium">#{c.frame ?? '?'}</span>
                  <span className="tnum text-muted">{c.score}</span>
                </span>
                {c.isBase && <span className="bg-panel px-1.5 pb-1 text-[10px] text-muted">{t('besttake.base')}</span>}
                {disabled && <span className="px-1.5 pb-1 text-[10px] leading-tight text-danger">{t(`besttake.reason_${c.reason ?? 'unknown'}`, { defaultValue: c.reason ?? '' })}</span>}
              </button>
            )
          })}
        </div>
        {person.candidates.length > shown.length || all ? (
          <button type="button" className="btn btn-ghost mt-2 !h-7 text-xs" onClick={() => setAll((v) => !v)} data-testid="bt-show-all">
            {all ? t('besttake.showTop') : t('besttake.showAll', { n: person.candidates.length })}
          </button>
        ) : null}
      </div>

      {patch && person.person_id !== null && (
        <div className="flex flex-col gap-1.5" data-testid="bt-blend">
          <Slider
            label={t('besttake.blend')}
            value={Math.round((patch.amount ?? 1) * 100)}
            min={0}
            max={100}
            step={1}
            def={100}
            unit="%"
            testId="bt-amount"
            onLive={(v) => setBlend('amount', v / 100)}
            onCommit={() => commitLive(qc, t('history.bestTakeBlend', { who }), `bt:${person.person_id}:blend`)}
          />
          <Slider
            label={t('besttake.feather')}
            value={Math.round((patch.feather ?? 0) * 100)}
            min={0}
            max={50}
            step={1}
            def={8}
            unit="%"
            testId="bt-feather"
            onLive={(v) => setBlend('feather', v / 100)}
            onCommit={() => commitLive(qc, t('history.bestTakeBlend', { who }), `bt:${person.person_id}:feather`)}
          />
        </div>
      )}

      {retargeted && <div className="rounded-control bg-warning/10 p-2 text-xs text-warning">{t('besttake.retargeted')}</div>}

      {warnings.length > 0 && (
        <ul className="flex flex-col gap-1" data-testid="bt-warnings">
          {warnings.map((w) => (
            <li key={w} className="flex items-start gap-1.5 rounded-control bg-warning/10 p-2 text-xs text-warning" data-warning={w}>
              <AlertTriangle size={13} className="mt-0.5 shrink-0" />
              {t(`besttake.warn_${w}`, { frame: frameOfPatch ?? '?' })}
            </li>
          ))}
        </ul>
      )}
      {result && !result.ok && (
        <div className="flex items-start gap-1.5 rounded-control bg-danger/10 p-2 text-xs text-danger" data-testid="bt-failure">
          <AlertTriangle size={13} className="mt-0.5 shrink-0" />
          {t('besttake.failed', { reason: t(`besttake.reason_${result.reason ?? 'unknown'}`, { defaultValue: result.reason ?? '' }) })}
        </div>
      )}

      <button type="button" className="btn w-full justify-start" disabled={!person.replaced} onClick={() => onRestore(person)} data-testid="bt-restore">
        <RotateCcw size={14} />
        {t('besttake.restore')}
      </button>
    </div>
  )
}
