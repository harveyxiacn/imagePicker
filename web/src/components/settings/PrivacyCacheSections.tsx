import { useQueryClient } from '@tanstack/react-query'
import { Loader2, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { api } from '@/api/client'
import { useCacheInfo, useHardware } from '@/api/queries'
import { CACHE_KINDS, type CacheKind, type GroupStrictness, type RenderBackend } from '@/api/types'
import { qk } from '@/lib/cache'
import { formatBytes } from '@/lib/format'
import { cacheFraction, isTypedConfirm, parseCacheGb } from '@/lib/settingsForm'
import { useFaceConsent } from '@/stores/faceConsent'
import { useToasts } from '@/stores/toasts'
import { Card, CommitInput, ConfirmDialog, Row, Select, Toggle } from './controls'
import { useSetting } from './useSetting'

const PROFILES = ['fast', 'standard'] as const
const STRICTNESS: GroupStrictness[] = ['loose', 'normal', 'strict']
const BACKENDS: RenderBackend[] = ['auto', 'gpu', 'cpu']

/** 分析 */
export function AnalysisSection() {
  const { t } = useTranslation()
  const { s, set } = useSetting()
  if (!s) return null
  return (
    <Card title={t('settings.analysis.title')} testId="settings-analysis">
      <Row label={t('settings.analysis.profile')} hint={t('settings.analysis.profileHint')}>
        <Select
          label={t('settings.analysis.profile')}
          testId="setting-analysis-profile"
          value={s.analysis.default_profile}
          options={PROFILES.map((p) => ({ value: p, label: t(`analysis.profile_${p}`) }))}
          onChange={(v) => set('analysis.default_profile', v)}
        />
      </Row>
      <Row label={t('settings.analysis.auto')} hint={t('settings.analysis.autoHint')}>
        <Toggle label={t('settings.analysis.auto')} testId="setting-analysis-auto" checked={s.analysis.auto_analyze_on_import} onChange={(v) => set('analysis.auto_analyze_on_import', v)} />
      </Row>
      <Row label={t('settings.analysis.strictness')} hint={t('settings.analysis.strictnessHint')}>
        <Select
          label={t('settings.analysis.strictness')}
          testId="setting-analysis-strictness"
          value={s.analysis.group_strictness}
          options={STRICTNESS.map((p) => ({ value: p, label: t(`settings.analysis.strictness_${p}`) }))}
          onChange={(v) => set('analysis.group_strictness', v)}
        />
      </Row>
    </Card>
  )
}

/** 人脸与隐私 */
export function FacesSection() {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const { s, set } = useSetting()
  const [ask, setAsk] = useState(false)
  const [typed, setTyped] = useState('')
  const [busy, setBusy] = useState(false)
  const word = t('settings.faces.confirmWord')
  if (!s) return null

  const wipe = async () => {
    setBusy(true)
    try {
      await api.deleteAllFaces()
      void qc.invalidateQueries({ queryKey: qk.peopleAll })
      void qc.invalidateQueries({ queryKey: qk.photosAll })
      void qc.invalidateQueries({ queryKey: ['photoPeople'] })
      useToasts.getState().push('success', t('settings.faces.wiped'), 3500)
      setAsk(false)
      setTyped('')
    } catch (e) {
      useToasts.getState().push('error', e instanceof Error ? e.message : String(e), 6000)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="flex flex-col gap-4" data-testid="settings-faces">
      <Card title={t('settings.faces.title')}>
        {/* switching on without consent first shows the purpose explanation (docs/02 §8) */}
        <Row label={t('settings.faces.enabled')} hint={t('settings.faces.enabledHint')}>
          <Toggle
            label={t('settings.faces.enabled')}
            testId="setting-faces-enabled"
            checked={s.faces.enabled}
            onChange={(v) => (v && !s.faces.consented ? useFaceConsent.getState().ask(null) : set('faces.enabled', v))}
          />
        </Row>
        <Row label={t('settings.faces.consent')} hint={t(s.faces.consented ? 'settings.faces.consentGivenHint' : 'settings.faces.consentMissingHint')} testId="faces-consent">
          <span className={`text-xs ${s.faces.consented ? 'text-success' : 'text-muted'}`}>{t(s.faces.consented ? 'settings.faces.consentGiven' : 'settings.faces.consentMissing')}</span>
          {s.faces.consented ? (
            <button className="btn" onClick={() => set('faces.consented', false)} data-testid="faces-consent-withdraw">
              {t('settings.faces.consentWithdraw')}
            </button>
          ) : (
            <button className="btn" onClick={() => useFaceConsent.getState().ask(null)} data-testid="faces-consent-ask">
              {t('settings.faces.consentAsk')}
            </button>
          )}
        </Row>
        <Row label={t('settings.faces.wipe')} hint={t('settings.faces.wipeHint')}>
          <button className="btn border-danger text-danger" onClick={() => setAsk(true)} data-testid="faces-wipe">
            <Trash2 size={13} />
            {t('settings.faces.wipe')}
          </button>
        </Row>
        <Row label={t('settings.privacy.network')} hint={t('settings.privacy.networkHint')}>
          <Toggle label={t('settings.privacy.network')} testId="setting-privacy-network" checked={s.privacy.allow_network} onChange={(v) => set('privacy.allow_network', v)} />
        </Row>
      </Card>
      <ConfirmDialog
        open={ask}
        onOpenChange={(o) => {
          setAsk(o)
          if (!o) setTyped('')
        }}
        title={t('settings.faces.wipeTitle')}
        description={t('settings.faces.wipeDesc')}
        confirmLabel={busy ? t('settings.faces.wiping') : t('settings.faces.wipe')}
        confirmDisabled={!isTypedConfirm(typed, word) || busy}
        danger
        testId="faces-wipe-confirm"
        onConfirm={() => void wipe()}
      >
        <label className="flex flex-col gap-1.5">
          <span className="text-muted">{t('settings.faces.typeToConfirm', { word })}</span>
          <input className="field" value={typed} onChange={(e) => setTyped(e.target.value)} placeholder={word} autoFocus spellCheck={false} data-testid="faces-wipe-input" />
        </label>
      </ConfirmDialog>
    </div>
  )
}

/** 缓存 */
export function CacheSection() {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const { s, set } = useSetting()
  const cache = useCacheInfo()
  const [busy, setBusy] = useState<string | null>(null)
  if (!s) return null
  const max = s.cache.max_gb

  const clear = async (kinds: CacheKind[]) => {
    setBusy(kinds.length > 1 ? 'all' : kinds[0])
    try {
      await api.clearCache(kinds)
      await qc.invalidateQueries({ queryKey: qk.cacheInfo })
      useToasts.getState().push('success', t('settings.cache.cleared'), 2500)
    } catch (e) {
      useToasts.getState().push('error', e instanceof Error ? e.message : String(e), 6000)
    } finally {
      setBusy(null)
    }
  }

  return (
    <Card title={t('settings.cache.title')} hint={t('settings.cache.hint')} testId="settings-cache">
      <div className="px-4 py-3" data-testid="cache-usage">
        <div className="mb-1.5 flex items-baseline justify-between">
          <span className="font-medium">{t('settings.cache.total')}</span>
          <span className="tnum text-muted">
            {cache.data ? formatBytes(cache.data.bytes) : '…'} / {max} GB
          </span>
        </div>
        <div className="h-2 overflow-hidden rounded bg-line">
          <div className="h-full bg-accent transition-[width] duration-300" style={{ width: `${Math.round(cacheFraction(cache.data?.bytes ?? 0, max) * 100)}%` }} />
        </div>
      </div>
      {CACHE_KINDS.map((k) => {
        const bytes = cache.data?.items[k] ?? 0
        return (
          <Row key={k} label={t(`settings.cache.kind_${k}`)} hint={t(`settings.cache.kind_${k}_hint`)} testId={`cache-kind-${k}`}>
            <span className="flex items-center gap-3">
              <span className="flex w-40 flex-col gap-1">
                <span className="tnum text-right text-xs text-muted">{formatBytes(bytes)}</span>
                <span className="h-1.5 overflow-hidden rounded bg-line">
                  <span className="block h-full bg-ai/80 transition-[width] duration-300" style={{ width: `${Math.round(cacheFraction(bytes, max) * 100 * 4)}%`, maxWidth: '100%' }} />
                </span>
              </span>
              <button className="btn" disabled={busy !== null || bytes === 0} onClick={() => void clear([k])} data-testid={`cache-clear-${k}`}>
                {busy === k ? <Loader2 size={13} className="animate-spin" /> : <Trash2 size={13} />}
                {t('settings.cache.clear')}
              </button>
            </span>
          </Row>
        )
      })}
      <Row label={t('settings.cache.max')} hint={t('settings.cache.maxHint')}>
        <CommitInput
          label={t('settings.cache.max')}
          testId="setting-cache-max"
          type="number"
          value={max}
          format={(v) => String(v)}
          parse={parseCacheGb}
          onCommit={(v) => set('cache.max_gb', v)}
          suffix="GB"
        />
        <button className="btn" disabled={busy !== null} onClick={() => void clear([...CACHE_KINDS])} data-testid="cache-clear-all">
          {busy === 'all' ? <Loader2 size={13} className="animate-spin" /> : <Trash2 size={13} />}
          {t('settings.cache.clearAll')}
        </button>
      </Row>
    </Card>
  )
}

/** 渲染 */
export function RenderSection() {
  const { t } = useTranslation()
  const { s, set } = useSetting()
  const hw = useHardware()
  if (!s) return null
  return (
    <Card title={t('settings.render.title')} testId="settings-render">
      <Row label={t('settings.render.backend')} hint={t('settings.render.backendHint', { gpu: hw.data?.gpu?.name ?? t('settings.render.noGpu') })}>
        <Select
          label={t('settings.render.backend')}
          testId="setting-render-backend"
          value={s.render.backend}
          options={BACKENDS.map((b) => ({ value: b, label: t(`settings.render.backend_${b}`) }))}
          onChange={(v) => set('render.backend', v)}
        />
      </Row>
    </Card>
  )
}
