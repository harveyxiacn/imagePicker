import { useQueryClient } from '@tanstack/react-query'
import { AlertTriangle, CheckCircle2, Link2Off, Loader2, QrCode, RefreshCw, ScanLine, Smartphone, Trash2, WifiOff } from 'lucide-react'
import { useEffect, useMemo, useReducer, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { api, ApiError } from '@/api/client'
import { useRemoteDevices, useRemoteStatus } from '@/api/queries'
import type { RemotePairStart, RemoteStatus } from '@/api/types'
import { qk } from '@/lib/cache'
import { getMobilePlatform } from '@/lib/mobilePlatform'
import {
  formatAgo,
  formatCode,
  formatCountdown,
  normalizeCode,
  PAIR_INITIAL,
  pairReducer,
  parsePairUri,
  secondsLeft,
  validatePairForm,
  type PairInput,
} from '@/lib/pairing'
import { useToasts } from '@/stores/toasts'
import { Card, Row } from './controls'

/** Error code -> dictionary key; unknown codes show the server message. */
function pairErrorText(e: unknown, t: (k: string) => string): string {
  if (e instanceof ApiError && ['invalid_code', 'pair_expired', 'host_unreachable', 'rate_limited', 'too_many_requests'].includes(e.code)) {
    return t(`remote.err_${e.code === 'too_many_requests' ? 'rate_limited' : e.code}`)
  }
  return e instanceof Error ? e.message : String(e)
}

/** Connected / offline / not paired summary of the paired host (phone side). Also used by the analysis profile chooser. */
export function RemoteStatusCard({ status, compact }: { status: RemoteStatus | undefined; compact?: boolean }) {
  const { t, i18n } = useTranslation()
  const [now] = useState(() => Date.now())
  if (!status) return null
  const tone = status.connected ? 'text-success' : status.paired ? 'text-warning' : 'text-muted'
  const Icon = status.connected ? CheckCircle2 : status.paired ? WifiOff : Link2Off
  const label = status.connected ? t('remote.state_connected') : status.paired ? t('remote.state_offline') : t('remote.state_unpaired')
  return (
    <div className="flex flex-col gap-1.5 rounded-control bg-bg/60 p-3" data-testid="remote-status" data-state={status.connected ? 'connected' : status.paired ? 'offline' : 'unpaired'}>
      <div className={`flex items-center gap-2 font-medium ${tone}`}>
        <Icon size={16} />
        {label}
        {status.host_tier && (
          <span className="ml-auto rounded bg-ai/20 px-1.5 text-xs text-ai" data-testid="remote-tier">
            {t('remote.hostTier', { tier: status.host_tier })}
          </span>
        )}
      </div>
      {status.paired && !compact && (
        <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-xs">
          <dt className="text-muted">{t('remote.host')}</dt>
          <dd className="truncate">{status.host_name ?? status.url}</dd>
          <dt className="text-muted">{t('remote.address')}</dt>
          <dd className="tnum truncate">{status.url}</dd>
          <dt className="text-muted">{t('remote.lastSeen')}</dt>
          <dd>{status.last_seen ? formatAgo(status.last_seen, now, i18n.language) : t('remote.never')}</dd>
        </dl>
      )}
      {status.paired && compact && status.host_name && <div className="text-xs text-muted">{status.host_name}</div>}
      {status.last_error && (
        <div className="flex items-start gap-1.5 text-xs text-danger" data-testid="remote-error">
          <AlertTriangle size={13} className="mt-0.5 shrink-0" />
          {t(`remote.err_${status.last_error}`, { defaultValue: status.last_error })}
        </div>
      )}
    </div>
  )
}

/** Settings: "远程 AI" - phone side. Pair by scanning a QR (when the platform has a scanner) or typing URL + 6-digit code. */
export function RemoteAiSection() {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const status = useRemoteStatus()
  const mp = useMemo(() => getMobilePlatform(), [])
  const [state, dispatch] = useReducer(pairReducer, PAIR_INITIAL)
  const [url, setUrl] = useState('')
  const [code, setCode] = useState('')
  const [touched, setTouched] = useState(false)
  const form = validatePairForm(url, code)
  const paired = status.data?.paired ?? false

  const connect = async (input: PairInput) => {
    dispatch({ type: 'submit', input })
    try {
      const s = await api.remoteConnect({ ...input, device_name: navigator.userAgent.includes('Android') ? 'Android' : 'Phone' })
      qc.setQueryData(qk.remoteStatus, s)
      dispatch({ type: 'success' })
      setCode('')
      useToasts.getState().push('success', t('remote.pairedToast'), 2500)
    } catch (e) {
      dispatch({ type: 'failure', error: pairErrorText(e, t as (k: string) => string) })
    }
  }

  const scanAndConnect = async () => {
    try {
      const text = await mp.scanQr!()
      if (!text) return
      const parsed = parsePairUri(text)
      if (!parsed) return void useToasts.getState().push('error', t('remote.err_bad_qr'), 5000)
      setUrl(parsed.url)
      setCode(parsed.code)
      await connect(parsed)
    } catch (e) {
      useToasts.getState().push('error', e instanceof Error ? e.message : String(e), 5000)
    }
  }

  const disconnect = async () => {
    try {
      await api.remoteDisconnect()
      dispatch({ type: 'disconnect' })
      await qc.invalidateQueries({ queryKey: qk.remoteStatus })
    } catch (e) {
      useToasts.getState().push('error', e instanceof Error ? e.message : String(e), 5000)
    }
  }

  const submit = () => {
    setTouched(true)
    if (form.value) void connect(form.value)
  }
  const busy = state.phase === 'submitting'

  return (
    <div className="flex flex-col gap-4" data-testid="remote-section">
      <p className="text-muted">{t('remote.intro')}</p>
      <Card title={t('remote.statusTitle')} testId="remote-status-card">
        <div className="p-3">
          {status.isPending ? (
            <Loader2 className="animate-spin text-muted" size={16} />
          ) : status.isError ? (
            <div className="text-danger">{(status.error as Error).message}</div>
          ) : (
            <RemoteStatusCard status={status.data} />
          )}
        </div>
        {paired && (
          <Row label={t('remote.disconnect')} hint={t('remote.disconnectHint')}>
            <button className="btn" onClick={() => void disconnect()} data-testid="remote-disconnect">
              <Link2Off size={14} />
              {t('remote.disconnect')}
            </button>
          </Row>
        )}
      </Card>

      {!paired && (
        <Card title={t('remote.pairTitle')} hint={t('remote.pairHint')} testId="remote-pair-card">
          {mp.scanQr && (
            <Row label={t('remote.scan')} hint={t('remote.scanHint')}>
              <button className="btn btn-primary" onClick={() => void scanAndConnect()} disabled={busy} data-testid="remote-scan">
                <ScanLine size={14} />
                {t('remote.scan')}
              </button>
            </Row>
          )}
          <form
            className="flex flex-col gap-3 p-4"
            noValidate
            onSubmit={(e) => {
              e.preventDefault()
              submit()
            }}
            data-testid="remote-form"
          >
            <div className="text-xs text-muted">{mp.scanQr ? t('remote.manualAlt') : t('remote.manual')}</div>
            <label className="flex flex-col gap-1">
              <span className="font-medium">{t('remote.url')}</span>
              <input
                className={`field !h-11 w-full ${touched && form.errors.url ? '!border-danger' : ''}`}
                type="text"
                inputMode="url"
                autoCapitalize="none"
                autoCorrect="off"
                placeholder="192.168.1.23:7878"
                value={url}
                onChange={(e) => {
                  setUrl(e.target.value)
                  dispatch({ type: 'edit' })
                }}
                aria-invalid={touched && !!form.errors.url}
                data-testid="remote-url"
              />
              {touched && form.errors.url && (
                <span className="text-xs text-danger" data-testid="remote-url-error">
                  {t(`remote.url_${form.errors.url}`)}
                </span>
              )}
            </label>
            <label className="flex flex-col gap-1">
              <span className="font-medium">{t('remote.code')}</span>
              <input
                className={`field !h-11 w-full text-center font-mono text-xl tracking-[0.4em] ${touched && form.errors.code ? '!border-danger' : ''}`}
                inputMode="numeric"
                autoComplete="one-time-code"
                maxLength={7}
                placeholder="••••••"
                value={code}
                onChange={(e) => {
                  setCode(normalizeCode(e.target.value))
                  dispatch({ type: 'edit' })
                }}
                aria-invalid={touched && !!form.errors.code}
                data-testid="remote-code"
              />
              {touched && form.errors.code && (
                <span className="text-xs text-danger" data-testid="remote-code-error">
                  {t(`remote.code_${form.errors.code}`)}
                </span>
              )}
            </label>
            {state.phase === 'idle' && state.error && (
              <div className="flex items-start gap-1.5 rounded-control border border-danger/40 bg-danger/10 p-2 text-danger" role="alert" data-testid="remote-pair-error">
                <AlertTriangle size={14} className="mt-0.5 shrink-0" />
                {state.error}
              </div>
            )}
            <button className={`btn ${mp.scanQr ? '' : 'btn-primary'} !h-11 justify-center`} type="submit" disabled={busy} data-testid="remote-connect">
              {busy ? <Loader2 size={15} className="animate-spin" /> : <Smartphone size={15} />}
              {busy ? t('remote.connecting') : t('remote.connect')}
            </button>
          </form>
        </Card>
      )}
    </div>
  )
}

/** Settings: "远程 AI 设备" - host side. Start pairing (code + QR + countdown), list devices, revoke. */
export function RemoteDevicesSection() {
  const { t, i18n } = useTranslation()
  const qc = useQueryClient()
  const devices = useRemoteDevices()
  const [pair, setPair] = useState<RemotePairStart | null>(null)
  const [busy, setBusy] = useState(false)
  const [now, setNow] = useState(() => Date.now())
  const [confirm, setConfirm] = useState<string | null>(null)

  useEffect(() => {
    if (!pair) return
    const id = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(id)
  }, [pair])

  const left = pair ? secondsLeft(pair.expires_at, now) : 0
  const expired = pair !== null && left === 0

  const start = async () => {
    setBusy(true)
    try {
      setPair(await api.remotePairStart())
      setNow(Date.now())
    } catch (e) {
      useToasts.getState().push('error', e instanceof Error ? e.message : String(e), 5000)
    } finally {
      setBusy(false)
    }
  }

  const revoke = async (id: string) => {
    try {
      await api.remoteRevoke(id)
      setConfirm(null)
      await qc.invalidateQueries({ queryKey: qk.remoteDevices })
    } catch (e) {
      useToasts.getState().push('error', e instanceof Error ? e.message : String(e), 5000)
    }
  }

  return (
    <div className="flex flex-col gap-4" data-testid="remote-devices-section">
      <p className="text-muted">{t('remote.hostIntro')}</p>
      <Card title={t('remote.pairNew')} testId="remote-host-pair">
        {!pair ? (
          <Row label={t('remote.pairStart')} hint={t('remote.pairStartHint')}>
            <button className="btn btn-primary" onClick={() => void start()} disabled={busy} data-testid="pair-start">
              {busy ? <Loader2 size={14} className="animate-spin" /> : <QrCode size={14} />}
              {t('remote.pairStart')}
            </button>
          </Row>
        ) : (
          <div className="flex flex-col items-center gap-3 p-4" data-testid="pair-panel" data-expired={expired}>
            <div className="text-xs text-muted">{t('remote.pairScanOrType')}</div>
            <img
              src={`data:image/svg+xml;utf8,${encodeURIComponent(pair.qr_svg)}`}
              alt={t('remote.qrAlt')}
              className={`h-44 w-44 rounded-lg bg-white p-1.5 ${expired ? 'opacity-30' : ''}`}
              data-testid="pair-qr"
            />
            <div className="tnum font-mono text-4xl font-bold tracking-[0.15em]" data-testid="pair-code">
              {formatCode(pair.code)}
            </div>
            <div className="tnum text-xs text-muted" data-testid="pair-url">
              {pair.url}
            </div>
            <div className={`tnum text-sm font-medium ${expired ? 'text-danger' : left < 60 ? 'text-warning' : 'text-success'}`} data-testid="pair-countdown">
              {expired ? t('remote.expired') : t('remote.expiresIn', { time: formatCountdown(left) })}
            </div>
            <button className="btn" onClick={() => void start()} disabled={busy} data-testid="pair-restart">
              <RefreshCw size={14} />
              {t('remote.pairRegenerate')}
            </button>
          </div>
        )}
      </Card>

      <Card title={t('remote.devices')} hint={t('remote.devicesHint')} testId="remote-devices">
        {devices.isPending ? (
          <div className="p-4">
            <Loader2 className="animate-spin text-muted" size={16} />
          </div>
        ) : devices.isError ? (
          <div className="p-4 text-danger">{(devices.error as Error).message}</div>
        ) : devices.data.length === 0 ? (
          <div className="p-4 text-muted" data-testid="devices-empty">
            {t('remote.noDevices')}
          </div>
        ) : (
          devices.data.map((d) => (
            <div key={d.device_id} className="flex items-center gap-3 px-4 py-3" data-testid={`device-${d.device_id}`}>
              <Smartphone size={18} className="shrink-0 text-muted" />
              <div className="min-w-0 flex-1">
                <div className="truncate font-medium">{d.name}</div>
                <div className="text-xs text-muted">
                  {t('remote.lastSeen')}: {d.last_seen ? formatAgo(d.last_seen, now, i18n.language) : t('remote.never')}
                </div>
              </div>
              {confirm === d.device_id ? (
                <span className="flex gap-1.5">
                  <button className="btn border-danger bg-danger text-white" onClick={() => void revoke(d.device_id)} data-testid={`revoke-confirm-${d.device_id}`}>
                    {t('remote.revokeConfirm')}
                  </button>
                  <button className="btn btn-ghost" onClick={() => setConfirm(null)}>
                    {t('common.cancel')}
                  </button>
                </span>
              ) : (
                <button className="btn" onClick={() => setConfirm(d.device_id)} data-testid={`revoke-${d.device_id}`}>
                  <Trash2 size={14} />
                  {t('remote.revoke')}
                </button>
              )}
            </div>
          ))
        )}
      </Card>
    </div>
  )
}
