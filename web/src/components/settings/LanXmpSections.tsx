import { useQueryClient } from '@tanstack/react-query'
import { AlertTriangle, Check, Copy, KeyRound, Loader2, Plus, RefreshCw, Upload, X } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { api } from '@/api/client'
import { useLan, usePatchSettings, useSessions } from '@/api/queries'
import type { XmpMode } from '@/api/types'
import { PASSWORD_MIN, passwordValid } from '@/lib/auth'
import { errorText } from '@/lib/errors'
import { qk } from '@/lib/cache'
import { parsePort } from '@/lib/settingsForm'
import { useToasts } from '@/stores/toasts'
import { Card, CommitInput, ConfirmDialog, Row, Toggle } from './controls'
import { useSetting } from './useSetting'

/** Owner (+ optional guest) password form: POST /api/auth/password {password, guest_password?} (8-256 chars, guest differs). */
function PasswordForm({ guestEnabled }: { guestEnabled: boolean }) {
  const { t } = useTranslation()
  const [owner, setOwner] = useState('')
  const [guest, setGuest] = useState('')
  const [busy, setBusy] = useState(false)
  const ownerOk = passwordValid(owner)
  const guestOk = guest === '' || (passwordValid(guest) && guest !== owner)
  const save = async (clearGuest = false) => {
    setBusy(true)
    try {
      await api.setPassword(owner, clearGuest ? '' : guest === '' ? undefined : guest)
      setOwner('')
      setGuest('')
      useToasts.getState().push('success', t('settings.lan.passwordSaved'), 2500)
    } catch (e) {
      useToasts.getState().push('error', errorText(e), 7000)
    } finally {
      setBusy(false)
    }
  }
  return (
    <>
      <Row label={t('settings.lan.password_owner')} hint={t('settings.lan.password_owner_hint', { n: PASSWORD_MIN })} testId="lan-password-owner">
        <input
          className="field w-44"
          type="password"
          autoComplete="new-password"
          value={owner}
          onChange={(e) => setOwner(e.target.value)}
          placeholder={t('settings.lan.passwordPlaceholder')}
          aria-label={t('settings.lan.password_owner')}
          data-testid="lan-password-input-owner"
        />
        {!guestEnabled && (
          <button className="btn" disabled={!ownerOk || busy} onClick={() => void save()} data-testid="lan-password-save-owner">
            {busy ? <Loader2 size={13} className="animate-spin" /> : <KeyRound size={13} />}
            {t('settings.lan.setPassword')}
          </button>
        )}
      </Row>
      {guestEnabled && (
        <Row label={t('settings.lan.password_guest')} hint={t('settings.lan.password_guest_hint', { n: PASSWORD_MIN })} testId="lan-password-guest">
          <input
            className={`field w-44 ${guestOk ? '' : '!border-danger'}`}
            type="password"
            autoComplete="new-password"
            value={guest}
            onChange={(e) => setGuest(e.target.value)}
            placeholder={t('settings.lan.guestPlaceholder')}
            aria-label={t('settings.lan.password_guest')}
            data-testid="lan-password-input-guest"
          />
          <button className="btn" disabled={!ownerOk || !guestOk || busy} onClick={() => void save()} data-testid="lan-password-save-owner" title={t('settings.lan.needOwner')}>
            {busy ? <Loader2 size={13} className="animate-spin" /> : <KeyRound size={13} />}
            {t('settings.lan.setPassword')}
          </button>
          <button className="btn btn-ghost" disabled={!ownerOk || busy} onClick={() => void save(true)} title={t('settings.lan.needOwner')} data-testid="lan-guest-clear">
            {t('settings.lan.clearGuest')}
          </button>
        </Row>
      )}
    </>
  )
}

/** Allowed roots (path whitelist for import / export / LUT / fs browsing). */
function RootsCard() {
  const { t } = useTranslation()
  const { s, set } = useSetting()
  const [draft, setDraft] = useState('')
  if (!s) return null
  const add = () => {
    const p = draft.trim()
    if (!p || s.roots.includes(p)) return
    set('roots', [...s.roots, p])
    setDraft('')
  }
  return (
    <Card title={t('settings.roots.title')} hint={t('settings.roots.hint')} testId="settings-roots">
      {s.roots.map((r) => (
        <div key={r} className="flex items-center gap-2 px-4 py-2">
          <code className="min-w-0 flex-1 truncate text-xs">{r}</code>
          <button className="btn btn-ghost btn-icon !h-6 !w-6" aria-label={t('settings.roots.remove')} title={t('settings.roots.remove')} onClick={() => set('roots', s.roots.filter((x) => x !== r))}>
            <X size={13} />
          </button>
        </div>
      ))}
      <div className="flex gap-2 px-4 py-3">
        <input className="field min-w-0 flex-1 font-mono text-xs" value={draft} onChange={(e) => setDraft(e.target.value)} placeholder={t('settings.roots.placeholder')} aria-label={t('settings.roots.add')} data-testid="roots-input" onKeyDown={(e) => e.key === 'Enter' && add()} />
        <button className="btn" disabled={!draft.trim()} onClick={add} data-testid="roots-add">
          <Plus size={13} />
          {t('settings.roots.add')}
        </button>
      </div>
    </Card>
  )
}

/** 局域网 / WebUI */
export function LanSection() {
  const { t } = useTranslation()
  const { s, set } = useSetting()
  const lan = useLan(true)
  const [copied, setCopied] = useState<string | null>(null)
  if (!s) return null

  const copy = (url: string) => {
    void navigator.clipboard?.writeText(url).then(() => {
      setCopied(url)
      setTimeout(() => setCopied(null), 1500)
    })
  }
  // an <img> never executes script, so the server-provided SVG is rendered as a data URI
  const qr = lan.data?.qr_svg ? `data:image/svg+xml;charset=utf-8,${encodeURIComponent(lan.data.qr_svg)}` : null

  return (
    <div className="flex flex-col gap-4" data-testid="settings-lan">
      <Card title={t('settings.lan.title')} hint={t('settings.lan.hint')}>
        <Row label={t('settings.lan.enabled')} hint={t('settings.lan.enabledHint')}>
          <Toggle label={t('settings.lan.enabled')} testId="setting-lan-enabled" checked={s.lan.enabled} onChange={(v) => set('lan.enabled', v)} />
        </Row>
        {lan.data?.restart_required && (
          <div className="flex items-center gap-2 bg-accent/10 px-4 py-2 text-xs text-accent" data-testid="lan-restart">
            <RefreshCw size={13} />
            {t('settings.lan.restartRequired')}
          </div>
        )}
        <Row label={t('settings.lan.port')} hint={t('settings.lan.portHint')}>
          <CommitInput
            label={t('settings.lan.port')}
            testId="setting-lan-port"
            type="number"
            value={s.lan.port}
            format={(v) => String(v)}
            parse={parsePort}
            onCommit={(v) => set('lan.port', v)}
          />
        </Row>
        <Row label={t('settings.lan.guest')} hint={t('settings.lan.guestHint')}>
          <Toggle label={t('settings.lan.guest')} testId="setting-lan-guest" checked={s.lan.guest_enabled} onChange={(v) => set('lan.guest_enabled', v)} />
        </Row>
        <PasswordForm guestEnabled={s.lan.guest_enabled} />
        <div className="flex items-start gap-2 bg-warning/10 px-4 py-2 text-xs text-warning">
          <AlertTriangle size={14} className="mt-0.5 shrink-0" />
          {t('settings.lan.warning')}
        </div>
      </Card>

      <RootsCard />

      {s.lan.enabled && (
        <Card title={t('settings.lan.access')} hint={t('settings.lan.accessHint')} testId="lan-access">
          <div className="flex flex-wrap items-start gap-6 px-4 py-4">
            <div className="flex min-w-0 flex-1 basis-64 flex-col gap-2" data-testid="lan-urls">
              {lan.isPending && <Loader2 size={14} className="animate-spin text-muted" />}
              {(lan.data?.urls ?? []).map((u) => (
                <div key={u} className="flex items-center gap-2 rounded-control border border-line bg-bg px-2.5 py-1.5">
                  <code className="min-w-0 flex-1 truncate text-xs">{u}</code>
                  <button className="btn btn-ghost btn-icon !h-6 !w-6" aria-label={t('settings.lan.copy')} title={t('settings.lan.copy')} onClick={() => copy(u)}>
                    {copied === u ? <Check size={13} className="text-success" /> : <Copy size={13} />}
                  </button>
                </div>
              ))}
              {lan.data && lan.data.urls.length === 0 && <div className="text-muted">{t('settings.lan.noUrls')}</div>}
            </div>
            {qr && (
              <figure className="flex flex-col items-center gap-1.5" data-testid="lan-qr">
                <img src={qr} alt={t('settings.lan.qr')} width={168} height={168} className="rounded-control border border-line bg-white p-1" />
                <figcaption className="text-xs text-muted">{t('settings.lan.qrHint')}</figcaption>
              </figure>
            )}
          </div>
        </Card>
      )}
    </div>
  )
}

const MODES: XmpMode[] = ['off', 'sidecar', 'modify_originals']

/** 互通: XMP sidecars (Lightroom / darktable). `modify_originals` rewrites original JPEGs and needs a second confirmation. */
export function XmpSection() {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const { s, set } = useSetting()
  const patch = usePatchSettings()
  const sessions = useSessions()
  const [sid, setSid] = useState<number | null>(null)
  const [busy, setBusy] = useState<'read' | 'write' | null>(null)
  const [confirmOriginals, setConfirmOriginals] = useState(false)
  if (!s) return null
  const sessionId = sid ?? sessions.data?.[0]?.id ?? null
  const choose = (m: XmpMode) => {
    if (m === s.xmp_mode) return
    if (m === 'modify_originals') setConfirmOriginals(true)
    else set('xmp_mode', m)
  }

  const sync = async (direction: 'read' | 'write') => {
    if (sessionId === null) return
    setBusy(direction)
    try {
      const r = await api.xmpSync(sessionId, direction)
      useToasts.getState().push('success', t(`settings.xmp.synced_${direction}`, { n: r.updated ?? 0 }), 3500)
      void qc.invalidateQueries({ queryKey: qk.photosAll })
    } catch (e) {
      useToasts.getState().push('error', e instanceof Error ? e.message : String(e), 6000)
    } finally {
      setBusy(null)
    }
  }

  return (
    <div className="flex flex-col gap-4" data-testid="settings-xmp">
      <Card title={t('settings.xmp.title')} hint={t('settings.xmp.hint')}>
        <div className="flex flex-col gap-2 px-4 py-3" role="radiogroup" aria-label={t('settings.xmp.title')}>
          {MODES.map((m) => (
            <label key={m} className={`flex cursor-pointer items-start gap-2.5 rounded-control border px-3 py-2 ${s.xmp_mode === m ? (m === 'modify_originals' ? 'border-warning bg-warning/10' : 'border-accent bg-accent/10') : 'border-line'}`}>
              <input type="radio" name="xmp-mode" className="mt-1" checked={s.xmp_mode === m} onChange={() => choose(m)} data-testid={`xmp-mode-${m}`} />
              <span>
                <span className="flex items-center gap-1.5 font-medium">
                  {m === 'modify_originals' && <AlertTriangle size={13} className="text-warning" />}
                  {t(`settings.xmp.mode_${m}`)}
                </span>
                <span className="mt-0.5 block text-xs text-muted">{t(`settings.xmp.mode_${m}_desc`)}</span>
              </span>
            </label>
          ))}
        </div>
        <ConfirmDialog
          open={confirmOriginals}
          onOpenChange={setConfirmOriginals}
          title={t('settings.xmp.originalsTitle')}
          description={t('settings.xmp.originalsDesc')}
          confirmLabel={t('settings.xmp.originalsConfirm')}
          danger
          testId="xmp-originals-confirm"
          onConfirm={() => {
            setConfirmOriginals(false)
            patch.mutate({ xmp_mode: 'modify_originals', confirm_modify_originals: true })
          }}
        />
        <div className="px-4 py-3 text-xs leading-relaxed text-muted" data-testid="xmp-compat">
          <div className="mb-1 font-medium text-fg">{t('settings.xmp.compatTitle')}</div>
          <ul className="list-disc pl-4">
            <li>{t('settings.xmp.compat1')}</li>
            <li>{t('settings.xmp.compat2')}</li>
            <li>{t('settings.xmp.compat3')}</li>
          </ul>
        </div>
      </Card>

      <Card title={t('settings.xmp.syncTitle')} hint={t('settings.xmp.syncHint')}>
        <Row label={t('settings.xmp.session')}>
          <select className="field max-w-56" value={sessionId ?? ''} onChange={(e) => setSid(Number(e.target.value))} aria-label={t('settings.xmp.session')} data-testid="xmp-session">
            {(sessions.data ?? []).map((x) => (
              <option key={x.id} value={x.id}>
                {x.title}
              </option>
            ))}
          </select>
        </Row>
        <Row label={t('settings.xmp.syncNow')} hint={s.xmp_mode === 'off' ? t('settings.xmp.syncOff') : undefined}>
          <button className="btn" disabled={s.xmp_mode === 'off' || sessionId === null || busy !== null} onClick={() => void sync('read')} data-testid="xmp-sync-read">
            {busy === 'read' ? <Loader2 size={13} className="animate-spin" /> : <RefreshCw size={13} />}
            {t('settings.xmp.read')}
          </button>
          <button className="btn" disabled={s.xmp_mode === 'off' || sessionId === null || busy !== null} onClick={() => void sync('write')} data-testid="xmp-sync-write">
            {busy === 'write' ? <Loader2 size={13} className="animate-spin" /> : <Upload size={13} />}
            {t('settings.xmp.write')}
          </button>
        </Row>
      </Card>
    </div>
  )
}
