import { AlertTriangle, Eye, EyeOff, Smile, Sparkles } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { faceCropUrl } from '@/api/client'
import type { Face, PhotoAnalysis } from '@/api/types'
import { useFaceMenu } from './FaceMenu'

const BAR_KEYS = ['sharpness', 'exposure', 'noise', 'iqa', 'aesthetic', 'face', 'composition'] as const
const NEGATIVE_REASONS = new Set(['closed_eyes', 'blurry', 'overexposed', 'underexposed', 'noisy', 'tilted'])

/** Localised reason text; unknown keys fall back to the raw key so newer backends never show blanks. */
export function ReasonText({ r }: { r: PhotoAnalysis['reasons'][number] }) {
  const { t } = useTranslation()
  return <>{t(`reason.${r.key}`, { ...(r.params ?? {}), defaultValue: r.key })}</>
}

function Bar({ k, value, delta }: { k: (typeof BAR_KEYS)[number]; value: number; delta: number | undefined }) {
  const { t } = useTranslation()
  // `noise` is "lower is better": show cleanliness so every bar reads "longer = better".
  const shown = k === 'noise' ? 1 - value : value
  const pct = Math.round(shown * 100)
  const tone = pct >= 70 ? 'bg-success' : pct >= 45 ? 'bg-warning' : 'bg-danger'
  return (
    <div className="flex items-center gap-2" data-testid={`score-${k}`}>
      <span className="w-14 shrink-0 text-muted">{t(`score.${k}`)}</span>
      <div className="h-2 flex-1 overflow-hidden rounded bg-line" role="meter" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100} aria-label={t(`score.${k}`)}>
        <div className={`h-full ${tone}`} style={{ width: `${pct}%` }} />
      </div>
      <span className="tnum w-8 shrink-0 text-right text-xs">{pct}</span>
      <span className={`tnum w-10 shrink-0 text-right text-xs ${delta === undefined ? 'text-faint' : delta >= 0 ? 'text-success' : 'text-danger'}`} title={t('score.contribution')}>
        {delta === undefined ? '' : `${delta >= 0 ? '+' : '−'}${Math.abs(delta).toFixed(2)}`}
      </span>
    </div>
  )
}

/** Score breakdown: per-dimension bars with contributions, localised reasons. */
export function ScoreBreakdown({ a }: { a: PhotoAnalysis }) {
  const { t } = useTranslation()
  const deltas = new Map(a.contributions.map((c) => [c.key, c.delta]))
  return (
    <div className="flex flex-col gap-1.5" data-testid="score-breakdown">
      {a.ai_score !== null && (
        <div className="flex items-center justify-between">
          <span className="flex items-center gap-1 text-ai">
            <Sparkles size={13} />
            {t('score.total')}
          </span>
          <span className="tnum font-medium">{a.ai_score.toFixed(2)}</span>
        </div>
      )}
      {BAR_KEYS.map((k) => {
        const v = a.scores[k]
        return v === null ? null : <Bar key={k} k={k} value={v} delta={deltas.get(k)} />
      })}
      {a.reasons.length > 0 && (
        <ul className="mt-1 flex flex-col gap-1" data-testid="score-reasons">
          {a.reasons.map((r, i) => (
            <li key={`${r.key}${i}`} className={`flex items-start gap-1.5 ${NEGATIVE_REASONS.has(r.key) ? 'text-warning' : 'text-muted'}`}>
              {NEGATIVE_REASONS.has(r.key) ? <AlertTriangle size={13} className="mt-0.5 shrink-0" /> : <Sparkles size={13} className="mt-0.5 shrink-0 text-ai" />}
              <span>
                <ReasonText r={r} />
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}

/** One face: crop with open-eyes / smile indicators. */
export function FaceChip({ face, size = 44, onMenu }: { face: Face; size?: number; onMenu?: (e: React.MouseEvent, f: Face) => void }) {
  const { t } = useTranslation()
  const eyesOpen = (face.eyes_open ?? 1) >= 0.45
  const smiling = (face.smile ?? 0) >= 0.55
  const label = [
    face.person_name ?? t('face.unassigned'),
    `${t('face.eyes')} ${face.eyes_open?.toFixed(2) ?? '–'}`,
    `${t('face.smile')} ${face.smile?.toFixed(2) ?? '–'}`,
    face.gaze !== null ? `${t('face.gaze')} ${face.gaze.toFixed(2)}` : null,
  ]
    .filter(Boolean)
    .join(' · ')
  return (
    <button
      type="button"
      className="flex shrink-0 flex-col items-center gap-0.5 rounded-control p-0.5 hover:bg-panel"
      title={label}
      aria-label={label}
      data-testid="face-chip"
      onContextMenu={(e) => onMenu?.(e, face)}
      onClick={(e) => onMenu?.(e, face)}
    >
      <img
        src={faceCropUrl(face.id, 128)}
        alt=""
        draggable={false}
        className="rounded-control bg-bg object-cover"
        style={{ width: size, height: size, outline: eyesOpen ? 'none' : '2px solid var(--danger)', outlineOffset: -1 }}
      />
      <span className="flex items-center gap-1">
        {eyesOpen ? <Eye size={12} className="text-success" /> : <EyeOff size={12} className="text-danger" />}
        <Smile size={12} className={smiling ? 'text-success' : 'text-faint'} />
      </span>
      {face.person_name && <span className="max-w-[52px] truncate text-[10px] text-muted">{face.person_name}</span>}
    </button>
  )
}

export function FaceStrip({ faces }: { faces: Face[] }) {
  const menu = useFaceMenu()
  const subjects = faces.filter((f) => f.is_subject)
  const rest = faces.filter((f) => !f.is_subject)
  return (
    <div className="flex flex-wrap gap-1" data-testid="face-strip">
      {[...subjects, ...rest].map((f) => (
        <FaceChip key={f.id} face={f} onMenu={(e, face) => menu.open(e, face)} />
      ))}
      {menu.node}
    </div>
  )
}
