import { useQueryClient } from '@tanstack/react-query'
import { Eye, LogOut, User } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { api } from '@/api/client'
import { useMe } from '@/api/queries'
import { qk } from '@/lib/cache'
import { LOGIN_PATH, noteLogout } from '@/lib/auth'
import { useToasts } from '@/stores/toasts'

/** LAN mode only: "只读访客" badge for guests + account menu with logout. Renders nothing on the desktop. */
export function UserMenu() {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const me = useMe()
  const [open, setOpen] = useState(false)
  const wrap = useRef<HTMLDivElement>(null)

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
  }, [open])

  if (!me.data?.lan || !me.data.role) return null
  const guest = me.data.role === 'guest'

  const logout = async () => {
    setOpen(false)
    // leave first: pages that stay mounted would otherwise fire 401s that remember THIS route as the login target
    noteLogout()
    qc.setQueryData(qk.me, { role: null, lan: true })
    navigate(LOGIN_PATH, { replace: true })
    try {
      await api.logout()
      qc.removeQueries({ predicate: (q) => q.queryKey[0] !== 'auth' })
    } catch (e) {
      useToasts.getState().push('error', e instanceof Error ? e.message : String(e))
      void qc.invalidateQueries({ queryKey: qk.me })
    }
  }

  return (
    <div className="relative flex items-center gap-1.5" ref={wrap}>
      {guest && (
        <span className="flex items-center gap-1 rounded-control bg-warning/15 px-2 py-0.5 text-xs font-medium text-warning" data-testid="guest-badge">
          <Eye size={12} />
          {t('auth.guestBadge')}
        </span>
      )}
      <button
        className="btn btn-ghost btn-icon"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={t('auth.menu')}
        title={t('auth.menu')}
        onClick={() => setOpen((o) => !o)}
        data-testid="user-menu"
      >
        <User size={15} />
      </button>
      {open && (
        <div role="menu" className="anim-pop absolute top-full right-0 z-50 mt-1 w-48 rounded-card border border-line bg-elevated p-1 shadow-[var(--shadow)]">
          <div className="px-2.5 py-1.5 text-xs text-muted">{guest ? t('auth.role_guest') : t('auth.role_owner')}</div>
          <button role="menuitem" className="flex w-full items-center gap-2 rounded-control px-2.5 py-1.5 text-left hover:bg-panel" onClick={() => void logout()} data-testid="logout">
            <LogOut size={14} />
            {t('auth.logout')}
          </button>
        </div>
      )}
    </div>
  )
}
