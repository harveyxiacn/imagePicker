import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Loader2, RefreshCw, RotateCcw, Sparkles } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { api } from '@/api/client'
import { useTaste } from '@/api/queries'
import type { Taste, TasteTrait } from '@/api/types'
import { qk } from '@/lib/cache'
import { tasteFeatureLabel, tasteLead, tasteNeeds, tasteStateText } from '@/lib/taste'
import { useToasts } from '@/stores/toasts'
import { Card, ConfirmDialog, Row, Toggle } from './controls'

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

/** One learned preference as a sentence ("Photos with more “Open eyes” tend to be your keepers"). */
function TraitText({ tr }: { tr: TasteTrait }) {
  const { t } = useTranslation()
  const feature = String(tr.params?.feature ?? '')
  const l = tasteFeatureLabel(feature)
  const name = t(l.key, { ...(l.params?.scene ? { scene: t(`scene_type.${l.params.scene}`) } : {}), defaultValue: feature })
  return <>{t(tr.key === 'dislikes' ? 'taste.trait_dislikes' : 'taste.trait_likes', { feature: name })}</>
}

/**
 * "My style" card (Settings → My style, and the /taste page): status, cold-start progress,
 * quality against the generic score, what it learned, retrain, forget and the on/off switch.
 */
export function TasteSection() {
  const { t, i18n } = useTranslation()
  const qc = useQueryClient()
  const q = useTaste()
  const [confirm, setConfirm] = useState(false)
  const [optOut, setOptOut] = useState(false)
  const done = (d?: Taste) => {
    if (d) qc.setQueryData(qk.taste, d)
    void qc.invalidateQueries({ queryKey: qk.taste })
  }
  const onError = (e: unknown) => useToasts.getState().push('error', errText(e), 5000)
  const toggle = useMutation({ mutationFn: (on: boolean) => api.patchTaste(on), onSuccess: done, onError })
  const train = useMutation({
    // 202: the training runs in the background; `taste.updated` refreshes the card when it is done
    mutationFn: api.trainTaste,
    onSuccess: (d) => {
      done(d)
      useToasts.getState().push('info', t('taste.trainStarted'), 2500)
    },
    onError,
  })
  const reset = useMutation({
    mutationFn: (disable: boolean) => api.resetTaste(disable),
    onSuccess: () => {
      done()
      useToasts.getState().push('success', t('taste.resetDone'), 2500)
    },
    onError,
  })

  if (q.isPending) {
    return (
      <div className="flex items-center gap-2 text-muted">
        <Loader2 className="animate-spin" size={16} />
        {t('library.loading')}
      </div>
    )
  }
  if (q.isError || !q.data) {
    return (
      <div className="flex items-center gap-3 text-danger">
        {errText(q.error)}
        <button className="btn" onClick={() => void q.refetch()}>
          {t('common.retry')}
        </button>
      </div>
    )
  }
  const d = q.data
  const needs = tasteNeeds(d)
  const st = tasteStateText(d)
  const m = d.metrics
  const pct = (v: number) => `${Math.round(v * 100)}%`
  const fmt = (n: number) => n.toLocaleString(i18n.language)
  const busy = toggle.isPending || train.isPending || reset.isPending
  const training = train.isPending || d.training

  return (
    <div className="flex flex-col gap-4" data-testid="taste-section">
      <Card title={t('taste.title')} hint={t('taste.intro')} testId="taste-card">
        <Row label={t('taste.enable')} hint={t('taste.enableHint')}>
          <Toggle checked={d.enabled} onChange={(v) => toggle.mutate(v)} label={t('taste.enable')} disabled={busy} testId="taste-enable" />
        </Row>

        <div className="flex items-center gap-3 px-4 py-3" data-testid="taste-state" data-state={d.state}>
          <Sparkles size={20} className={d.active ? 'shrink-0 text-ai' : 'shrink-0 text-faint'} aria-hidden />
          <div className="min-w-0 flex-1">
            <div className="font-medium" data-testid="taste-state-text">
              {t(st.title)}
            </div>
            <div className="text-xs text-muted">{t(st.hint, st.params)}</div>
          </div>
        </div>

        {d.legacy_labels > 0 && (
          <p className="px-4 py-2 text-xs text-muted" data-testid="taste-legacy">
            {t('taste.legacyNote', { n: fmt(d.legacy_labels) })}
          </p>
        )}
        <Row
          label={t('taste.progress')}
          hint={t('taste.progressHint', { keep: fmt(d.positives), reject: fmt(d.negatives), min: d.min_per_class })}
          testId="taste-labels"
        >
          <span className="tnum text-sm">{fmt(d.labels)}</span>
        </Row>
        {(needs.keep > 0 || needs.reject > 0) && (
          <div className="px-4 pb-3">
            <div
              className="h-1.5 overflow-hidden rounded bg-line"
              role="progressbar"
              aria-label={t('taste.progress')}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={Math.round(needs.fraction * 100)}
            >
              <div className="h-full bg-ai transition-[width] duration-300" style={{ width: `${Math.round(needs.fraction * 100)}%` }} />
            </div>
          </div>
        )}

        <Row label={t('taste.quality')} hint={m ? t('taste.qualityHint', { n: fmt(m.eval_positives + m.eval_negatives), folds: m.folds }) : t('taste.qualityNone')} testId="taste-accuracy">
          {m && (
            <div className="tnum text-right text-sm">
              <div>{t('taste.auc', { taste: m.auc.toFixed(2), base: m.base_auc.toFixed(2) })}</div>
              <div className={`text-xs ${tasteLead(m) > 0 ? 'text-success' : 'text-muted'}`}>{t('taste.lead', { n: tasteLead(m) > 0 ? `+${tasteLead(m)}` : tasteLead(m) })}</div>
              <div className="text-xs text-muted">{t('taste.accuracyVs', { taste: pct(m.accuracy), base: pct(m.base_accuracy) })}</div>
            </div>
          )}
        </Row>

        <Row label={t('taste.retrain')} hint={t('taste.retrainHint', { n: d.pending, every: d.retrain_every })}>
          <button className="btn" onClick={() => train.mutate()} disabled={!d.enabled || busy || d.training} aria-busy={training} data-testid="taste-retrain">
            {training ? <Loader2 size={14} className="animate-spin" aria-hidden /> : <RefreshCw size={14} aria-hidden />}
            {training ? t('taste.training') : t('taste.retrain')}
          </button>
        </Row>

        <Row label={t('taste.reset')} hint={t('taste.resetHint')}>
          <button
            className="btn"
            onClick={() => {
              setOptOut(false)
              setConfirm(true)
            }}
            disabled={busy || (d.labels === 0 && !d.metrics)}
            data-testid="taste-reset"
          >
            <RotateCcw size={14} />
            {t('taste.reset')}
          </button>
        </Row>
      </Card>

      <Card title={t('taste.traits')} testId="taste-traits-card">
        {d.traits.length === 0 ? (
          <p className="px-4 py-3 text-muted" data-testid="taste-no-traits">
            {t('taste.noTraits')}
          </p>
        ) : (
          <ul className="flex flex-col gap-1.5 px-4 py-3" data-testid="taste-traits">
            {d.traits.map((tr, i) => (
              <li key={`${tr.key}-${String(tr.params?.feature ?? i)}`} className="flex items-center gap-2" data-testid="taste-trait">
                <Sparkles size={13} className="shrink-0 text-ai" aria-hidden />
                <TraitText tr={tr} />
              </li>
            ))}
          </ul>
        )}
      </Card>

      <ConfirmDialog
        open={confirm}
        onOpenChange={setConfirm}
        title={t('taste.resetTitle')}
        description={t('taste.resetDesc')}
        confirmLabel={t('taste.reset')}
        danger
        onConfirm={() => {
          setConfirm(false)
          reset.mutate(optOut)
        }}
        testId="taste-reset-confirm"
      >
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={optOut} onChange={(e) => setOptOut(e.target.checked)} data-testid="taste-reset-optout" />
          {t('taste.resetOptOut')}
        </label>
      </ConfirmDialog>
    </div>
  )
}
