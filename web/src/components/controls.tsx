import { Ban, Flag as FlagIcon, Star } from 'lucide-react'
import { useId } from 'react'
import { useTranslation } from 'react-i18next'
import type { ColorLabel, Flag } from '@/api/types'

import { starFills } from '@/lib/ai'
import { COLOR_NAMES, colorVar } from '@/lib/colors'

/** Clickable 5-star control. `value` null/0 = none. Clicking the current value clears it. */
export function StarRating({
  value,
  onChange,
  size = 16,
}: {
  value: number | null
  onChange?: (v: number | null) => void
  size?: number
}) {
  const { t } = useTranslation()
  const v = value ?? 0
  return (
    <div className="flex items-center gap-0.5" role="group" aria-label={t('inspector.rating')}>
      {[1, 2, 3, 4, 5].map((n) => (
        <button
          key={n}
          type="button"
          className="rounded p-0.5 text-accent hover:scale-110 disabled:cursor-default"
          style={{ transition: 'transform 120ms' }}
          aria-label={t('inspector.starN', { n })}
          title={`${n} (${n})`}
          disabled={!onChange}
          onClick={() => onChange?.(v === n ? null : n)}
        >
          <Star size={size} fill={n <= v ? 'currentColor' : 'none'} className={n <= v ? '' : 'text-faint'} />
        </button>
      ))}
    </div>
  )
}

/** One star: hollow purple outline for AI ratings; `half` fills the left half. */
function AiStar({ fill, size }: { fill: 'full' | 'half' | 'empty'; size: number }) {
  const path = 'M12 2.5l2.9 6.1 6.6.8-4.9 4.6 1.3 6.6L12 17.3 6.1 20.6l1.3-6.6L2.5 9.4l6.6-.8z'
  const id = useId()
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden className="shrink-0">
      {fill === 'half' && (
        <defs>
          <clipPath id={id}>
            <rect x="0" y="0" width="12" height="24" />
          </clipPath>
        </defs>
      )}
      <path d={path} fill="none" stroke="currentColor" strokeWidth={fill === 'empty' ? 1.5 : 2} strokeLinejoin="round" opacity={fill === 'empty' ? 0.35 : 1} strokeDasharray={fill === 'empty' ? '2.5 2.5' : undefined} />
      {fill === 'half' && <path d={path} fill="currentColor" clipPath={`url(#${id})`} opacity="0.85" />}
    </svg>
  )
}

/**
 * AI rating (0.5 steps): hollow purple stars, visually distinct from the solid amber user stars.
 * `value` null = not analysed yet.
 */
export function AiStars({ value, size = 14, showValue = true }: { value: number | null; size?: number; showValue?: boolean }) {
  const { t } = useTranslation()
  return (
    <div className="flex items-center gap-1 text-ai" title={value === null ? t('inspector.aiPending') : `${t('inspector.aiRating')} ${value.toFixed(1)}`} data-testid="ai-stars">
      <span aria-hidden>✨</span>
      <div className="flex gap-0.5">
        {starFills(value).map((f, i) => (
          <AiStar key={i} fill={f} size={size} />
        ))}
      </div>
      {showValue && <span className="tnum ml-0.5 text-xs text-muted">{value === null ? t('inspector.aiPending') : value.toFixed(1)}</span>}
    </div>
  )
}

export function FlagButtons({ value, onChange }: { value: Flag; onChange: (f: Flag) => void }) {
  const { t } = useTranslation()
  return (
    <div className="flex gap-1">
      <button
        className="btn"
        aria-pressed={value === 1}
        title={`${t('flag.picked')} (P)`}
        onClick={() => onChange(value === 1 ? 0 : 1)}
      >
        <FlagIcon size={14} className="text-success" fill={value === 1 ? 'currentColor' : 'none'} />
        {t('flag.picked')}
      </button>
      <button
        className="btn"
        aria-pressed={value === -1}
        title={`${t('flag.rejected')} (X)`}
        onClick={() => onChange(value === -1 ? 0 : -1)}
      >
        <Ban size={14} className="text-danger" />
        {t('flag.rejected')}
      </button>
    </div>
  )
}

export function ColorDots({ value, onChange }: { value: ColorLabel; onChange: (c: ColorLabel) => void }) {
  const { t } = useTranslation()
  const keyFor: Record<string, string> = { red: '6', yellow: '7', green: '8', blue: '9' }
  return (
    <div className="flex items-center gap-1.5">
      {COLOR_NAMES.map((c) => (
        <button
          key={c}
          aria-label={t(`color.${c}`)}
          aria-pressed={value === c}
          title={keyFor[c] ? `${t(`color.${c}`)} (${keyFor[c]})` : t(`color.${c}`)}
          className="h-5 w-5 rounded-full border-2 transition-transform hover:scale-110"
          style={{
            background: colorVar(c),
            borderColor: value === c ? 'var(--fg)' : 'transparent',
            boxShadow: value === c ? '0 0 0 1px var(--bg)' : undefined,
          }}
          onClick={() => onChange(value === c ? null : c)}
        />
      ))}
    </div>
  )
}
