import { useMutation, useQueryClient } from '@tanstack/react-query'
import { ChevronLeft, HeartHandshake, Loader2, RotateCcw, Sparkles } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useNavigate } from 'react-router-dom'
import { api } from '@/api/client'
import { useTaste } from '@/api/queries'
import { HeaderControls } from '@/components/HeaderControls'
import { Modal } from '@/components/Modal'
import { qk } from '@/lib/cache'
import { TASTE_MIN_LABELS, TASTE_RETRAIN_EVERY, tasteProgress } from '@/lib/taste'
import { useToasts } from '@/stores/toasts'

function Stat({ label, value, testId, hint }: { label: string; value: React.ReactNode; testId: string; hint?: string }) {
  return (
    <div className="rounded-card border border-line bg-panel p-3" data-testid={testId}>
      <div className="text-xs text-muted">{label}</div>
      <div className="tnum mt-1 text-xl font-semibold">{value}</div>
      {hint && <div className="mt-1 text-[11px] text-faint">{hint}</div>}
    </div>
  )
}

/** "My taste" (F2.10): what the personalised scoring has learnt from your ratings and picks. */
export function Taste() {
  const { t, i18n } = useTranslation()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const q = useTaste()
  const [confirm, setConfirm] = useState(false)
  const reset = useMutation({
    mutationFn: api.resetTaste,
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: qk.taste })
      useToasts.getState().push('success', t('taste.resetDone'), 2500)
    },
    onError: (e) => useToasts.getState().push('error', e instanceof Error ? e.message : String(e), 5000),
  })
  const d = q.data
  const progress = d ? tasteProgress(d.labels) : null
  const fmt = (n: number) => n.toLocaleString(i18n.language)

  return (
    <div className="flex h-full flex-col" data-testid="taste-page">
      <header className="flex h-12 shrink-0 items-center gap-3 border-b border-line px-3">
        <button className="btn btn-ghost" onClick={() => (window.history.length > 1 ? navigate(-1) : navigate('/'))} aria-label={t('common.back')}>
          <ChevronLeft size={16} />
          <span className="hidden sm:inline">{t('common.back')}</span>
        </button>
        <h1 className="flex items-center gap-2 text-base font-semibold">
          <HeartHandshake size={16} />
          {t('taste.title')}
        </h1>
        <div className="ml-auto flex items-center gap-2">
          <HeaderControls />
        </div>
      </header>

      <main className="min-h-0 flex-1 overflow-y-auto p-4">
        {q.isPending ? (
          <div className="flex h-full items-center justify-center gap-2 text-muted">
            <Loader2 className="animate-spin" size={18} />
            {t('library.loading')}
          </div>
        ) : q.isError ? (
          <div className="flex h-full flex-col items-center justify-center gap-3 text-danger">
            {(q.error as Error).message}
            <button className="btn" onClick={() => void q.refetch()}>
              {t('common.retry')}
            </button>
          </div>
        ) : (
          d &&
          progress && (
            <div className="mx-auto flex max-w-3xl flex-col gap-5">
              <p className="text-muted">{t('taste.intro')}</p>

              <div className="flex items-center gap-3 rounded-card border p-3" style={{ borderColor: d.active ? 'var(--ai)' : 'var(--line)' }} data-testid="taste-state">
                <Sparkles size={20} className={d.active ? 'text-ai' : 'text-faint'} />
                <div className="min-w-0 flex-1">
                  <div className="font-medium" data-testid="taste-state-text">
                    {d.active ? t('taste.active') : t('taste.inactive')}
                  </div>
                  <div className="text-xs text-muted">{d.active ? t('taste.activeHint', { alpha: Math.round(d.alpha * 100) }) : t('taste.inactiveHint', { n: TASTE_MIN_LABELS })}</div>
                </div>
              </div>

              <div className="grid gap-3 sm:grid-cols-3">
                <Stat label={t('taste.labels')} value={fmt(d.labels)} testId="taste-labels" hint={t('taste.nextTrain', { n: progress.toNext, every: TASTE_RETRAIN_EVERY })} />
                <Stat label={t('taste.alpha')} value={d.alpha.toFixed(2)} testId="taste-alpha" hint={t('taste.alphaHint')} />
                <Stat
                  label={t('taste.accuracy')}
                  value={d.holdout_accuracy === null ? '–' : `${Math.round(d.holdout_accuracy * 100)}%`}
                  testId="taste-accuracy"
                  hint={t('taste.accuracyHint')}
                />
              </div>

              <div className="h-1.5 overflow-hidden rounded bg-line" aria-hidden>
                <div className="h-full bg-ai transition-[width] duration-300" style={{ width: `${Math.round(progress.fraction * 100)}%` }} />
              </div>

              <section>
                <h2 className="mb-2 font-medium">{t('taste.traits')}</h2>
                {d.traits.length === 0 ? (
                  <p className="text-muted" data-testid="taste-no-traits">
                    {t('taste.noTraits', { n: TASTE_MIN_LABELS })}
                  </p>
                ) : (
                  <ul className="flex flex-col gap-1.5" data-testid="taste-traits">
                    {d.traits.map((tr) => (
                      <li key={tr.key} className="flex items-center gap-2 rounded-control border border-line bg-panel px-3 py-2" data-testid="taste-trait">
                        <Sparkles size={13} className="shrink-0 text-ai" />
                        {t(`taste.trait_${tr.key}`, { ...tr.params, defaultValue: tr.key })}
                      </li>
                    ))}
                  </ul>
                )}
              </section>

              <div className="flex items-center gap-3 border-t border-line pt-4">
                <button className="btn" onClick={() => setConfirm(true)} disabled={d.labels === 0 || reset.isPending} data-testid="taste-reset">
                  <RotateCcw size={14} />
                  {t('taste.reset')}
                </button>
                <span className="text-xs text-faint">{t('taste.resetHint')}</span>
                <Link to="/" className="btn btn-ghost ml-auto">
                  {t('taste.backHome')}
                </Link>
              </div>
            </div>
          )
        )}
      </main>

      <Modal
        open={confirm}
        onOpenChange={setConfirm}
        title={t('taste.resetTitle')}
        description={t('taste.resetDesc')}
        width="max-w-sm"
        footer={
          <>
            <button className="btn" onClick={() => setConfirm(false)}>
              {t('common.cancel')}
            </button>
            <button
              className="btn btn-primary"
              onClick={() => {
                setConfirm(false)
                reset.mutate()
              }}
              data-testid="taste-reset-confirm"
            >
              {t('taste.reset')}
            </button>
          </>
        }
      >
        <span className="sr-only">{t('taste.resetTitle')}</span>
      </Modal>
    </div>
  )
}
