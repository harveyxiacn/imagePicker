import { Images, Settings as SettingsIcon, Sparkles, Users } from 'lucide-react'
import { useEffect } from 'react'
import { useTranslation } from 'react-i18next'
import { useLocation, useNavigate } from 'react-router-dom'
import { useMe, useSessions } from '@/api/queries'
import { can } from '@/lib/auth'
import { activeNavTab, NAV_TABS, navTarget, sessionFromPath, type NavTab } from '@/lib/layout'
import { rememberSession, lastSession } from '@/lib/lastSession'
import { useUi } from '@/stores/ui'

const ICONS: Record<NavTab, typeof Images> = { library: Images, cull: Sparkles, people: Users, settings: SettingsIcon }

/** Phone bottom navigation: 图库 / 挑片 / 人物 / 设置 (doc 08 section 5). Shown below 768 px only. */
export function BottomNav() {
  const { t } = useTranslation()
  const { pathname } = useLocation()
  const navigate = useNavigate()
  const view = useUi((s) => s.view)
  const role = useMe().data?.role ?? 'owner'
  const sessions = useSessions()

  const inPath = sessionFromPath(pathname)
  useEffect(() => {
    if (inPath !== null) rememberSession(inPath)
  }, [inPath])
  // Fall back to the newest session when nothing was opened yet.
  const sessionId = inPath ?? lastSession() ?? sessions.data?.[0]?.id ?? null
  const active = activeNavTab(pathname, view)

  const go = (tab: NavTab) => {
    const to = navTarget(tab, sessionId)
    if (!to) return
    const ui = useUi.getState()
    if (tab === 'library') ui.setView('grid')
    if (tab === 'cull') ui.setView('loupe')
    // Switching the view inside the same library page keeps its state; other routes reset it on mount.
    if (pathname !== to) navigate(to, { state: { view: tab === 'cull' ? 'loupe' : tab === 'library' ? 'grid' : undefined } })
  }

  return (
    <nav
      className="flex shrink-0 border-t border-line bg-panel"
      style={{ paddingBottom: 'env(safe-area-inset-bottom, 0px)', paddingLeft: 'env(safe-area-inset-left, 0px)', paddingRight: 'env(safe-area-inset-right, 0px)' }}
      aria-label={t('mobile.nav.label')}
      data-testid="bottom-nav"
    >
      {NAV_TABS.filter((tab) => tab !== 'settings' || can(role, 'settings')).map((tab) => {
        const Icon = ICONS[tab]
        const disabled = navTarget(tab, sessionId) === null
        const on = active === tab
        return (
          <button
            key={tab}
            className={`flex min-h-[52px] flex-1 flex-col items-center justify-center gap-0.5 text-[11px] transition-colors disabled:opacity-40 ${on ? 'text-accent' : 'text-muted'}`}
            aria-current={on ? 'page' : undefined}
            disabled={disabled}
            onClick={() => go(tab)}
            data-testid={`nav-${tab}`}
          >
            <Icon size={21} strokeWidth={on ? 2.4 : 1.8} />
            <span className={on ? 'font-semibold' : ''}>{t(`mobile.nav.${tab}`)}</span>
          </button>
        )
      })}
    </nav>
  )
}
