import { Ban, Flag as FlagIcon, Star } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import type { ColorLabel, Flag } from '@/api/types'

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

/** AI rating slot (M1: always empty, dashed purple). */
export function AiRatingSlot({ value }: { value: number | null }) {
  const { t } = useTranslation()
  return (
    <div className="flex items-center gap-1 text-ai" title={t('inspector.aiPending')}>
      <span aria-hidden>✨</span>
      <div className="flex gap-0.5">
        {[1, 2, 3, 4, 5].map((n) => (
          <Star
            key={n}
            size={14}
            strokeDasharray="2 2"
            fill={value !== null && n <= value ? 'currentColor' : 'none'}
            className="opacity-50"
          />
        ))}
      </div>
      <span className="ml-1 text-xs text-muted">{value === null ? t('inspector.aiPending') : value.toFixed(1)}</span>
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
