import { useQueryClient } from '@tanstack/react-query'
import { Cpu, Loader2, Sparkles } from 'lucide-react'
import { useEffect, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import { useAnalysisStatus, useHardware } from '@/api/queries'
import type { AnalysisProfile, WorkerState } from '@/api/types'
import { analysisFraction, startAnalysis } from '@/lib/analysis'
import { useAnalysisUi } from '@/stores/analysis'
import { useUi } from '@/stores/ui'

const PROFILES: AnalysisProfile[] = ['fast', 'standard']

const STATE_DOT: Record<WorkerState, string> = {
  stopped: 'bg-faint',
  starting: 'bg-warning',
  ready: 'bg-success',
  busy: 'bg-ai',
  crashed: 'bg-danger',
  unavailable: 'bg-danger',
}

/** Hardware / worker status chip for the top bar: tier, GPU name and worker state. */
export function WorkerChip() {
  const { t } = useTranslation()
  const hw = useHardware()
  if (!hw.data) return null
  const w = hw.data
  const label = w.gpu?.name.replace(/^NVIDIA GeForce /, '') ?? w.device ?? 'CPU'
  const title = [
    `${t('worker.state')}: ${t(`worker.state_${w.state}`)}`,
    w.tier ? `${t('worker.tier')} ${w.tier}` : null,
    w.gpu ? `${w.gpu.name} · ${(w.gpu.vram_mb / 1024).toFixed(0)} GB` : null,
    w.providers.length ? w.providers.join(', ') : null,
    w.error,
  ]
    .filter(Boolean)
    .join('\n')
  return (
    <span className="hidden items-center gap-1.5 rounded-control border border-line px-2 py-0.5 text-xs text-muted xl:flex" title={title} data-testid="worker-chip">
      <span className={`h-2 w-2 rounded-full ${STATE_DOT[w.state]} ${w.state === 'busy' || w.state === 'starting' ? 'animate-pulse' : ''}`} />
      <Cpu size={12} />
      {w.tier && <span className="font-semibold text-fg">{w.tier}</span>}
      <span className="max-w-32 truncate">{label}</span>
    </span>
  )
}

interface Props {
  sessionId: number
  photoCount: number
}

/** "✨ One-click analysis": profile picker popover; shows live progress while a run is active. */
export function AnalyzeControl({ sessionId, photoCount }: Props) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const open = useAnalysisUi((s) => s.menuOpen)
  const setOpen = useAnalysisUi((s) => s.setMenuOpen)
  const profile = useAnalysisUi((s) => s.profile)
  const setProfile = useAnalysisUi((s) => s.setProfile)
  const selected = useUi((s) => s.selection.ids)
  const status = useAnalysisStatus(sessionId)
  const hw = useHardware()
  const wrap = useRef<HTMLDivElement>(null)
  const onlySelected = useRef(false)
  const running = status.data?.state === 'running'

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (wrap.current && !wrap.current.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(false)
    document.addEventListener('mousedown', onDown)
    window.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      window.removeEventListener('keydown', onKey)
    }
  }, [open, setOpen])

  const start = async () => {
    setOpen(false)
    const ids = onlySelected.current && selected.size > 0 ? [...selected] : undefined
    await startAnalysis(qc, sessionId, profile, ids)
  }

  const pct = status.data && running ? Math.round(analysisFraction(status.data) * 100) : 0

  return (
    <div className="relative" ref={wrap}>
      <button
        className="btn btn-ai"
        disabled={photoCount === 0}
        onClick={() => setOpen(!open)}
        aria-haspopup="dialog"
        aria-expanded={open}
        title={t('top.analyzeHint')}
        data-testid="analyze-button"
      >
        {running ? <Loader2 size={14} className="animate-spin" /> : <Sparkles size={14} />}
        <span className="hidden sm:inline">{running ? t('analysis.runningShort', { pct }) : t('top.analyze')}</span>
      </button>
      {open && (
        <div
          role="dialog"
          aria-label={t('top.analyze')}
          className="anim-pop absolute top-full right-0 z-40 mt-1.5 w-[340px] max-w-[calc(100vw-24px)] rounded-card border border-line bg-elevated p-3 shadow-[var(--shadow)]"
          data-testid="analyze-menu"
        >
          <div className="mb-2 font-medium">{t('analysis.profile')}</div>
          <div className="flex flex-col gap-1.5" role="radiogroup" aria-label={t('analysis.profile')}>
            {PROFILES.map((p) => (
              <label
                key={p}
                className="flex cursor-pointer items-start gap-2 rounded-control border p-2"
                style={{ borderColor: profile === p ? 'var(--ai)' : 'var(--line)', background: profile === p ? 'color-mix(in srgb, var(--ai) 10%, transparent)' : undefined }}
              >
                <input type="radio" name="profile" className="mt-1" checked={profile === p} onChange={() => setProfile(p)} />
                <span>
                  <span className="font-medium">{t(`analysis.profile_${p}`)}</span>
                  <span className="block text-xs text-muted">{t(`analysis.profile_${p}_desc`)}</span>
                </span>
              </label>
            ))}
          </div>
          {selected.size > 0 && (
            <label className="mt-2 flex cursor-pointer items-center gap-2 text-muted">
              <input type="checkbox" onChange={(e) => (onlySelected.current = e.target.checked)} />
              {t('analysis.onlySelected', { n: selected.size })}
            </label>
          )}
          {hw.data && (
            <div className="mt-3 rounded-control bg-panel p-2 text-xs text-muted" data-testid="hardware-card">
              <div className="flex items-center gap-1.5 text-fg">
                <Cpu size={13} />
                {hw.data.gpu ? hw.data.gpu.name : t('worker.cpuOnly')}
                {hw.data.tier && <span className="rounded bg-ai/20 px-1 text-ai">{hw.data.tier}</span>}
              </div>
              <div className="mt-0.5">
                {t('worker.state')}: {t(`worker.state_${hw.data.state}`)}
                {hw.data.gpu && ` · ${(hw.data.gpu.vram_mb / 1024).toFixed(0)} GB`}
              </div>
              {hw.data.error && <div className="mt-0.5 text-danger">{hw.data.error}</div>}
            </div>
          )}
          <div className="mt-3 flex justify-end gap-2">
            <button className="btn btn-ghost" onClick={() => setOpen(false)}>
              {t('common.cancel')}
            </button>
            <button className="btn btn-primary" onClick={() => void start()} disabled={running} data-testid="analyze-start">
              <Sparkles size={14} />
              {t('analysis.start')}
            </button>
          </div>
        </div>
      )}
    </div>
  )
}
