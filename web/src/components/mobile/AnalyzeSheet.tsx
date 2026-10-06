import { useQueryClient } from '@tanstack/react-query'
import { Cpu, Loader2, Server, Sparkles } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'
import { useAnalysisStatus } from '@/api/queries'
import { useRemoteStatus } from '@/api/queries'
import type { AnalysisProfile } from '@/api/types'
import { analysisFraction, startAnalysis } from '@/lib/analysis'
import { useAnalysisUi } from '@/stores/analysis'
import { useUi } from '@/stores/ui'
import { RemoteStatusCard } from '../settings/RemoteAiSections'
import { Sheet } from './Sheet'

const PHONE_PROFILES: { id: AnalysisProfile; icon: typeof Cpu; label: string; desc: string }[] = [
  { id: 'lite', icon: Cpu, label: 'analysis.profile_lite', desc: 'analysis.profile_lite_desc' },
  { id: 'standard', icon: Server, label: 'analysis.profile_standard_remote', desc: 'analysis.profile_standard_remote_desc' },
]

/** Phone analysis entry: 快速（本机） or 标准（远程 AI，需配对）, with the paired host's status. */
export function AnalyzeSheet({ sessionId, photoCount }: { sessionId: number; photoCount: number }) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const open = useAnalysisUi((s) => s.menuOpen)
  const setOpen = useAnalysisUi((s) => s.setMenuOpen)
  const status = useAnalysisStatus(sessionId)
  const remote = useRemoteStatus()
  const selected = useUi((s) => s.selection.ids)
  const [profile, setProfile] = useState<AnalysisProfile>('lite')
  const [onlySelected, setOnlySelected] = useState(false)
  const running = status.data?.state === 'running'
  const pct = status.data && running ? Math.round(analysisFraction(status.data) * 100) : 0
  const needsPair = profile === 'standard' && !remote.data?.connected

  const start = async () => {
    setOpen(false)
    await startAnalysis(qc, sessionId, profile, onlySelected && selected.size > 0 ? [...selected] : undefined)
  }

  return (
    <>
      <button className={`btn btn-ai ${running ? '' : 'btn-icon'}`} disabled={photoCount === 0} onClick={() => setOpen(true)} aria-haspopup="dialog" aria-label={t('top.analyze')} data-testid="analyze-button">
        {running ? <Loader2 size={16} className="animate-spin" /> : <Sparkles size={16} />}
        {running && <span className="tnum">{pct}%</span>}
      </button>
      <Sheet
        open={open}
        onOpenChange={setOpen}
        title={t('top.analyze')}
        testId="analyze-sheet"
        footer={
          <button className="btn btn-primary !h-11 w-full justify-center" onClick={() => void start()} disabled={running || needsPair} data-testid="analyze-start">
            <Sparkles size={15} />
            {t('analysis.start')}
          </button>
        }
      >
        <div className="flex flex-col gap-2 p-4" role="radiogroup" aria-label={t('analysis.profile')} data-testid="profile-chooser">
          {PHONE_PROFILES.map(({ id, icon: Icon, label, desc }) => (
            <button
              key={id}
              role="radio"
              aria-checked={profile === id}
              className="flex items-start gap-3 rounded-card border p-3 text-left"
              style={{ borderColor: profile === id ? 'var(--ai)' : 'var(--line)', background: profile === id ? 'color-mix(in srgb, var(--ai) 10%, transparent)' : undefined }}
              onClick={() => setProfile(id)}
              data-testid={`profile-${id}`}
            >
              <Icon size={20} className={profile === id ? 'text-ai' : 'text-muted'} />
              <span className="flex-1">
                <span className="block text-base font-semibold">{t(label)}</span>
                <span className="mt-0.5 block text-xs text-muted">{t(desc)}</span>
              </span>
              <span className={`mt-1 h-4 w-4 shrink-0 rounded-full border-2 ${profile === id ? 'border-ai bg-ai' : 'border-faint'}`} />
            </button>
          ))}

          {profile === 'standard' && (
            <div className="flex flex-col gap-2" data-testid="remote-host-status">
              <div className="text-xs font-medium text-muted">{t('remote.statusTitle')}</div>
              <RemoteStatusCard status={remote.data} compact />
              {needsPair && (
                <div className="flex flex-col gap-2 rounded-control border border-warning/40 bg-warning/10 p-3 text-xs">
                  <span>{remote.data?.paired ? t('mobile.analyze.hostOffline') : t('mobile.analyze.needPair')}</span>
                  <Link to="/settings?section=remote" className="btn btn-primary w-fit" onClick={() => setOpen(false)} data-testid="go-pair">
                    {t('mobile.analyze.goPair')}
                  </Link>
                </div>
              )}
            </div>
          )}

          {selected.size > 0 && (
            <label className="flex min-h-11 cursor-pointer items-center gap-2 text-muted">
              <input type="checkbox" className="h-4 w-4" checked={onlySelected} onChange={(e) => setOnlySelected(e.target.checked)} />
              {t('analysis.onlySelected', { n: selected.size })}
            </label>
          )}
        </div>
      </Sheet>
    </>
  )
}
