import { useQueryClient } from '@tanstack/react-query'
import { Cpu, Download, Loader2, Sparkles } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useLocation } from 'react-router-dom'
import { api, ApiError } from '@/api/client'
import { useMe, useOnboarding } from '@/api/queries'
import { can } from '@/lib/auth'
import { qk } from '@/lib/cache'
import { installRuntimeAndWait, runtimeView } from '@/lib/runtime'
import { COACH_MARKS, shouldPostDone, visibleOverlay, type CoachMark, type OnboardingAction } from '@/lib/onboarding'
import { useOnboardingUi } from '@/stores/onboarding'
import { useToasts } from '@/stores/toasts'
import { Modal } from './Modal'

const mb = (n: number) => (n >= 1024 ? `${(n / 1024).toFixed(1)} GB` : `${Math.round(n)} MB`)

/** First run (doc 04 section 7): hardware card, then three coach marks inside the library. Mounted once in the app shell. */
export function Onboarding() {
  const qc = useQueryClient()
  const location = useLocation()
  const me = useMe()
  const allowed = can(me.data?.role ?? 'owner', 'settings') && location.pathname !== '/login'
  const info = useOnboarding(allowed)
  const step = useOnboardingUi((s) => s.step)
  const coach = useOnboardingUi((s) => s.coach)
  const downloading = useOnboardingUi((s) => s.downloading)
  const firstRun = allowed && (info.data?.first_run ?? false)

  // `first_run:false` from the server (done elsewhere): never show anything
  useEffect(() => {
    if (info.data && !info.data.first_run && step !== 'finished') useOnboardingUi.getState().dispatch({ type: 'alreadyDone' })
  }, [info.data, step])

  const send = (a: OnboardingAction) => {
    const prev = { step, coach, downloading }
    const next = useOnboardingUi.getState().dispatch(a)
    if (shouldPostDone(prev, next)) {
      void api
        .onboardingDone()
        .then(() => qc.setQueryData(qk.onboarding, info.data ? { ...info.data, first_run: false } : undefined))
        .catch((e) => useToasts.getState().push('error', e instanceof Error ? e.message : String(e)))
    }
    return next
  }

  const inLibrary = /^\/s\/\d+\/?$/.test(location.pathname)
  const overlay = visibleOverlay({ step, coach, downloading }, firstRun, inLibrary)
  if (!overlay || !info.data) return null
  if (overlay === 'card') return <HardwareCard info={info.data} onChoose={(download) => send({ type: download ? 'download' : 'basic' })} />
  return <CoachBubble mark={overlay} index={coach} onNext={() => send({ type: 'next' })} onBack={() => send({ type: 'back' })} onSkip={() => send({ type: 'skipTour' })} />
}

function HardwareCard({ info, onChoose }: { info: NonNullable<ReturnType<typeof useOnboarding>['data']>; onChoose: (download: boolean) => void }) {
  const { t } = useTranslation()
  const [busy, setBusy] = useState(false)
  const [percent, setPercent] = useState<number | null>(null)
  const gpu = info.hardware.gpu

  const download = async () => {
    setBusy(true)
    try {
      // M7: the AI runtime comes first (progress in the button), then the models
      let freshInstall = false
      try {
        const rt = await api.runtime()
        if (rt.state !== 'ready') {
          freshInstall = true
          await installRuntimeAndWait((r) => setPercent(runtimeView(r).percent))
          setPercent(null)
        }
      } catch (e) {
        if (!(e instanceof ApiError) || e.status !== 404) throw e // 404: a server without the runtime API
      }
      let ids = freshInstall ? undefined : info.recommended_models
      if (!ids) ids = (await api.models()).models.filter((m) => m.required_for.includes('standard') && !m.installed).map((m) => m.id)
      if (ids.length) {
        const { task_id } = await api.ensureModels(ids)
        useToasts.getState().updateTask({ type: 'task.progress', task_id, kind: 'model_download', done: 0, total: Math.round(info.recommended_download_mb * 1e6), state: 'running' })
      }
      onChoose(true)
    } catch (e) {
      useToasts.getState().push('error', e instanceof Error ? e.message : String(e), 6000)
    } finally {
      setBusy(false)
      setPercent(null)
    }
  }

  return (
    <Modal open onOpenChange={(o) => !o && onChoose(false)} title={t('onboarding.title')} description={t('onboarding.subtitle')} width="max-w-lg"
      footer={
        <>
          <button className="btn" onClick={() => onChoose(false)} data-testid="onboarding-basic">
            {t('onboarding.basic')}
          </button>
          <button className="btn btn-primary" disabled={busy} onClick={() => void download()} data-testid="onboarding-download">
            {busy ? <Loader2 size={14} className="animate-spin" /> : <Download size={14} />}
            {percent !== null ? `${t('runtime.installingHint')} ${percent}%` : t('onboarding.download')}
          </button>
        </>
      }
    >
      <div className="flex flex-col gap-3" data-testid="onboarding-card">
        <div className="flex items-center gap-3 rounded-card border border-line bg-bg/50 p-3">
          <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full bg-accent/15 text-accent">
            <Cpu size={20} />
          </span>
          <div className="min-w-0 flex-1">
            <div className="truncate font-medium">{gpu ? t('onboarding.detected', { gpu: gpu.name }) : t('onboarding.detectedCpu')}</div>
            <div className="text-xs text-muted">{gpu ? `${(gpu.vram_mb / 1024).toFixed(0)} GB VRAM` : (info.hardware.device ?? 'CPU')}</div>
          </div>
          <span className="rounded bg-accent/20 px-2 py-1 text-base font-semibold text-accent" data-testid="onboarding-tier">
            {info.recommended_tier}
          </span>
        </div>
        <ul className="flex flex-col gap-1.5 text-muted">
          <li className="flex gap-2">
            <Sparkles size={14} className="mt-0.5 shrink-0 text-ai" />
            {t('onboarding.tierDesc', { tier: info.recommended_tier })}
          </li>
          <li className="flex gap-2">
            <Download size={14} className="mt-0.5 shrink-0 text-ai" />
            <span>
              {t('onboarding.size', { size: mb(info.recommended_download_mb) })} <span className="text-faint">{t('onboarding.sizeHint')}</span>
            </span>
          </li>
        </ul>
      </div>
    </Modal>
  )
}

interface Rect {
  top: number
  left: number
  width: number
  height: number
}

function CoachBubble({ mark, index, onNext, onBack, onSkip }: { mark: CoachMark; index: number; onNext: () => void; onBack: () => void; onSkip: () => void }) {
  const { t } = useTranslation()
  const [rect, setRect] = useState<Rect | null>(null)

  useEffect(() => {
    const measure = () => {
      const el = document.querySelector(`[data-coach="${mark}"]`)
      if (!el) return setRect(null)
      const r = el.getBoundingClientRect()
      setRect(r.width === 0 && r.height === 0 ? null : { top: r.top, left: r.left, width: r.width, height: r.height })
    }
    const raf = requestAnimationFrame(measure)
    const timer = setInterval(measure, 300)
    window.addEventListener('resize', measure)
    return () => {
      cancelAnimationFrame(raf)
      clearInterval(timer)
      window.removeEventListener('resize', measure)
    }
  }, [mark])

  const W = 300
  const vw = typeof window === 'undefined' ? 1200 : window.innerWidth
  const vh = typeof window === 'undefined' ? 800 : window.innerHeight
  const below = !rect || rect.top + rect.height + 190 < vh
  const left = rect ? Math.min(Math.max(8, rect.left + rect.width / 2 - W / 2), vw - W - 8) : Math.max(8, vw / 2 - W / 2)
  const top = rect ? (below ? rect.top + rect.height + 12 : Math.max(8, rect.top - 12 - 150)) : Math.max(8, vh - 220)
  const arrowLeft = rect ? Math.min(Math.max(16, rect.left + rect.width / 2 - left), W - 16) : null
  const last = index === COACH_MARKS.length - 1

  return (
    <>
      {rect && <div className="pointer-events-none fixed z-[70] rounded-control border-2 border-accent shadow-[0_0_0_4px_color-mix(in_srgb,var(--accent)_25%,transparent)]" style={{ top: rect.top - 3, left: rect.left - 3, width: rect.width + 6, height: rect.height + 6 }} />}
      <div className="anim-pop fixed z-[71] rounded-card border border-accent bg-elevated p-3 shadow-[var(--shadow)]" style={{ top, left, width: W }} role="status" data-testid="coach-bubble" data-mark={mark}>
        {arrowLeft !== null && (
          <span
            className={`absolute h-3 w-3 rotate-45 border-accent bg-elevated ${below ? '-top-1.5 border-t border-l' : '-bottom-1.5 border-r border-b'}`}
            style={{ left: arrowLeft - 6 }}
          />
        )}
        <div className="mb-1 flex items-center gap-1.5 font-semibold">
          <Sparkles size={13} className="text-accent" />
          {t(`onboarding.coach_${mark}_title`)}
        </div>
        <p className="text-muted">{t(`onboarding.coach_${mark}`)}</p>
        <div className="mt-2.5 flex items-center gap-2">
          <span className="tnum text-xs text-faint">
            {index + 1} / {COACH_MARKS.length}
          </span>
          <button className="btn btn-ghost ml-auto !h-6 text-xs" onClick={onSkip} data-testid="coach-skip">
            {t('onboarding.skip')}
          </button>
          {index > 0 && (
            <button className="btn !h-6 text-xs" onClick={onBack}>
              {t('common.back')}
            </button>
          )}
          <button className="btn btn-primary !h-6 text-xs" onClick={onNext} data-testid="coach-next">
            {last ? t('onboarding.done') : t('onboarding.next')}
          </button>
        </div>
      </div>
    </>
  )
}
