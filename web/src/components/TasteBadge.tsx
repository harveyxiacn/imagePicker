import { ArrowDown, ArrowUp, ChevronDown, ChevronRight, HeartHandshake, Loader2 } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { api } from '@/api/client'
import { useTasteExplain } from '@/api/queries'
import type { Photo, Taste, TasteContribution } from '@/api/types'
import { qk } from '@/lib/cache'
import { splitContributions, tasteFeatureLabel, tastePercent } from '@/lib/taste'
import { useToasts } from '@/stores/toasts'
import { errorText } from '@/lib/errors'

/** Small "my style" score chip (grid / loupe); nothing while no taste model is in use. */
export function TasteChip({ score, className = '' }: { score: number | null | undefined; className?: string }) {
  const { t } = useTranslation()
  if (score === null || score === undefined) return null
  const pct = tastePercent(score)
  return (
    <span
      className={`tnum inline-flex items-center gap-1 rounded bg-ai/15 px-1.5 py-0.5 text-xs font-medium text-ai ${className}`}
      title={t('taste.scoreHint', { pct })}
      data-testid="taste-chip-score"
    >
      <HeartHandshake size={12} aria-hidden />
      <span aria-hidden>{pct}%</span>
      <span className="sr-only">{t('taste.scoreHint', { pct })}</span>
    </span>
  )
}

function Reason({ c }: { c: TasteContribution }) {
  const { t } = useTranslation()
  const l = tasteFeatureLabel(c.feature)
  const params = l.params?.scene ? { scene: t(`scene_type.${l.params.scene}`) } : undefined
  const name = t(l.key, { ...params, defaultValue: c.feature })
  const Arrow = c.direction === 'high' ? ArrowUp : ArrowDown
  return (
    <li className="flex items-center gap-1.5" data-testid="taste-reason">
      <Arrow size={12} className="shrink-0 text-muted" role="img" aria-label={t(c.direction === 'high' ? 'taste.dirHigh' : 'taste.dirLow')} />
      <span>{name}</span>
    </li>
  )
}

/** Inspector row: the photo's taste score and, on demand, why (top features for and against). */
export function TasteBadge({ photo }: { photo: Photo }) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const [open, setOpen] = useState(false)
  const ex = useTasteExplain(photo.id, open)
  const forget = useMutation({
    mutationFn: () => api.forgetTaste({ photo_id: photo.id }),
    onSuccess: (d: Taste) => {
      qc.setQueryData(qk.taste, d)
      void qc.invalidateQueries({ queryKey: qk.taste })
      void qc.invalidateQueries({ queryKey: qk.photosAll })
      useToasts.getState().push('success', t('taste.forgotPhoto'), 2500)
    },
    onError: (e: unknown) =>
      useToasts.getState().push('error', errorText(e), 5000),
  })
  if (photo.taste_score === null || photo.taste_score === undefined) return null
  const data = ex.data && ex.data.photo_id === photo.id ? ex.data : undefined
  const { pro, con } = splitContributions(data?.contributions ?? [])
  const panelId = `taste-why-${photo.id}`
  return (
    <div className="flex flex-col gap-1.5" data-testid="taste-badge">
      <div className="flex items-center justify-between gap-2">
        <span className="text-muted">{t('taste.title')}</span>
        <div className="flex items-center gap-1.5">
          <TasteChip score={photo.taste_score} />
          <button
            className="btn btn-ghost !h-6 !px-1.5 text-xs"
            onClick={() => setOpen((o) => !o)}
            aria-expanded={open}
            aria-controls={panelId}
            data-testid="taste-why"
          >
            {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
            {t('taste.why')}
          </button>
        </div>
      </div>
      {open && (
        <div id={panelId} className="rounded-control border border-line bg-panel p-2 text-xs" data-testid="taste-why-panel">
          {ex.isPending ? (
            <Loader2 size={14} className="animate-spin text-muted" aria-label={t('library.loading')} />
          ) : ex.isError ? (
            <span className="text-danger">{errorText(ex.error)}</span>
          ) : pro.length + con.length === 0 ? (
            <span className="text-muted">{t('taste.whyNone')}</span>
          ) : (
            <div className="grid grid-cols-2 gap-2">
              <div>
                <div className="mb-1 font-medium text-success">{t('taste.whyPro')}</div>
                <ul className="flex flex-col gap-0.5">
                  {pro.map((c) => (
                    <Reason key={c.feature} c={c} />
                  ))}
                </ul>
              </div>
              <div>
                <div className="mb-1 font-medium text-danger">{t('taste.whyCon')}</div>
                <ul className="flex flex-col gap-0.5">
                  {con.map((c) => (
                    <Reason key={c.feature} c={c} />
                  ))}
                </ul>
              </div>
            </div>
          )}
          <button
            className="btn mt-2 !h-6 !px-1.5 text-xs"
            title={t('taste.forgetPhotoHint')}
            disabled={forget.isPending}
            onClick={() => forget.mutate()}
            data-testid="taste-forget-photo"
          >
            {forget.isPending && <Loader2 size={12} className="animate-spin" />}
            {t('taste.forgetPhoto')}
          </button>
        </div>
      )}
    </div>
  )
}
