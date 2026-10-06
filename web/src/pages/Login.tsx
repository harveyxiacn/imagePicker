import { useQueryClient } from '@tanstack/react-query'
import { Aperture, Eye, Loader2, Lock } from 'lucide-react'
import { useEffect, useState, type FormEvent } from 'react'
import { useTranslation } from 'react-i18next'
import { Navigate, useNavigate, useSearchParams } from 'react-router-dom'
import { api, ApiError } from '@/api/client'
import { useMe } from '@/api/queries'
import { HeaderControls } from '@/components/HeaderControls'
import { loginErrorKey, retryAfterOf, sanitizeNext } from '@/lib/auth'

/** LAN-mode login (route `/login`). After success the user returns to the route that triggered the 401. */
export function Login() {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const [params] = useSearchParams()
  const me = useMe()
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<'rateLimited' | 'wrong' | 'lanDisabled' | 'failed' | null>(null)
  const [wait, setWait] = useState(0)

  // rate limit countdown (Retry-After / retry_after)
  useEffect(() => {
    if (wait <= 0) return
    const id = setTimeout(() => setWait((w) => w - 1), 1000)
    return () => clearTimeout(id)
  }, [wait])

  const submit = async (e: FormEvent) => {
    e.preventDefault()
    if (!password || busy || wait > 0) return
    setBusy(true)
    setError(null)
    try {
      await api.login(password)
      // drop everything fetched as the anonymous caller, then go back where we came from
      await qc.invalidateQueries()
      navigate(sanitizeNext(params.get('next')), { replace: true })
    } catch (err) {
      const api = err instanceof ApiError ? err : null
      setError(loginErrorKey(api?.status ?? 0, api?.code))
      if (api?.status === 429) setWait(retryAfterOf(api.body))
      setPassword('')
    } finally {
      setBusy(false)
    }
  }

  // already signed in (or not in LAN mode at all): nothing to do here
  if (me.data && (me.data.role || !me.data.lan)) return <Navigate to={sanitizeNext(params.get('next'))} replace />

  return (
    <div className="flex h-full flex-col" data-testid="login-page">
      <header className="flex h-12 shrink-0 items-center justify-between border-b border-line px-4">
        <div className="flex items-center gap-2 text-base font-semibold">
          <Aperture size={20} className="text-accent" />
          imagePicker
        </div>
        <HeaderControls />
      </header>
      <main className="flex min-h-0 flex-1 items-center justify-center overflow-auto p-4">
        <form onSubmit={(e) => void submit(e)} className="anim-pop flex w-full max-w-sm flex-col gap-4 rounded-card border border-line bg-panel p-6 shadow-[var(--shadow)]">
          <div className="flex flex-col items-center gap-2 text-center">
            <span className="flex h-11 w-11 items-center justify-center rounded-full bg-accent/15 text-accent">
              <Lock size={20} />
            </span>
            <h1 className="text-xl font-semibold">{t('login.title')}</h1>
            <p className="text-muted">{t('login.subtitle')}</p>
          </div>
          <label className="flex flex-col gap-1.5">
            <span className="text-muted">{t('login.password')}</span>
            <input
              className={`field !h-9 ${error ? '!border-danger' : ''}`}
              type="password"
              autoComplete="current-password"
              autoFocus
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              aria-invalid={error !== null}
              data-testid="login-password"
            />
          </label>
          {error && (
            <div className="rounded-control bg-danger/10 px-3 py-2 text-danger" role="alert" data-testid="login-error" data-kind={error}>
              {error === 'rateLimited' && wait > 0 ? t('login.error_rateLimitedWait', { n: wait }) : t(`login.error_${error}`)}
            </div>
          )}
          <button className="btn btn-primary !h-9 justify-center" type="submit" disabled={!password || busy || wait > 0} data-testid="login-submit">
            {busy && <Loader2 size={14} className="animate-spin" />}
            {t('login.submit')}
          </button>
          <div className="flex items-start gap-2 rounded-control bg-bg/60 p-2.5 text-xs text-muted" data-testid="login-guest-hint">
            <Eye size={14} className="mt-0.5 shrink-0" />
            {t('login.guestHint')}
          </div>
        </form>
      </main>
    </div>
  )
}
