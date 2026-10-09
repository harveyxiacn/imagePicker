import { useEffect } from 'react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { BrowserRouter, Navigate, Route, Routes, useLocation, useNavigate } from 'react-router-dom'
import { authHooks } from '@/api/client'
import { startEvents } from '@/api/events'
import { useMe, useSettings } from '@/api/queries'
import { can, LOGIN_PATH, loginTarget, setRole, shouldRedirectOn401 } from '@/lib/auth'
import { qk } from '@/lib/cache'
import { showsBottomNav } from '@/lib/layout'
import { useIsMobile } from '@/lib/useLayout'
import { onAssistantDone } from '@/lib/assistantRun'
import { handleXmpConflict } from '@/lib/xmp'
import { Login } from '@/pages/Login'
import { Settings } from '@/pages/Settings'
import { BottomNav } from '@/components/mobile/BottomNav'
import { CatalogRecovery } from '@/components/CatalogRecovery'
import { FaceConsentDialog } from '@/components/FaceConsentDialog'
import { Onboarding } from '@/components/Onboarding'
import { useAssistant } from '@/stores/assistant'
import { genOnDone, genOnTask } from '@/lib/gen'
import { BestTake } from '@/pages/BestTake'
import { Home } from '@/pages/Home'
import { Edit } from '@/pages/Edit'
import { Library } from '@/pages/Library'
import { People } from '@/pages/People'
import { Taste } from '@/pages/Taste'
import { Toasts } from '@/components/Toasts'
import { useToasts } from '@/stores/toasts'
import { useUi } from '@/stores/ui'
import i18n from '@/i18n'

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { refetchOnWindowFocus: false, staleTime: 60_000, retry: 1 },
  },
})

function useTheme() {
  const theme = useUi((s) => s.theme)
  useEffect(() => {
    const mq = window.matchMedia('(prefers-color-scheme: light)')
    const apply = () => {
      const light = theme === 'light' || (theme === 'system' && mq.matches)
      document.documentElement.dataset.theme = light ? 'light' : 'dark'
    }
    apply()
    mq.addEventListener('change', apply)
    return () => mq.removeEventListener('change', apply)
  }, [theme])
}

function useLang() {
  const lang = useUi((s) => s.lang)
  useEffect(() => {
    void i18n.changeLanguage(lang)
    document.documentElement.lang = lang
  }, [lang])
}

/** LAN auth: any 401 (or `role:null` from /auth/me) goes to /login once; the login page returns to the previous route. */
function useAuthGuard() {
  const navigate = useNavigate()
  const location = useLocation()
  const me = useMe()

  // module-level role for non-React code (actions, shortcuts); the desktop app always reports `owner`
  useEffect(() => {
    setRole(me.data ? me.data.role : 'owner')
  }, [me.data])

  useEffect(() => {
    authHooks.onUnauthorized = (requestPath) => {
      if (!shouldRedirectOn401(requestPath, window.location.pathname)) return
      queryClient.setQueryData(qk.me, { role: null, lan: true })
      navigate(loginTarget(window.location), { replace: true })
    }
    return () => {
      authHooks.onUnauthorized = null
    }
  }, [navigate])

  useEffect(() => {
    if (me.data?.lan && me.data.role === null && location.pathname !== LOGIN_PATH) navigate(loginTarget(location), { replace: true })
  }, [me.data, location, navigate])
}

/** Server settings are the source of truth for language / theme (owner only: guests cannot read settings). */
function useAppearanceSync() {
  const me = useMe()
  const settings = useSettings(can(me.data?.role ?? null, 'settings'))
  const lang = settings.data?.language
  const theme = settings.data?.theme
  useEffect(() => {
    if (lang && useUi.getState().lang !== lang) useUi.getState().setLang(lang)
  }, [lang])
  useEffect(() => {
    if (theme && useUi.getState().theme !== theme) useUi.getState().setTheme(theme)
  }, [theme])
}

function Shell() {
  useTheme()
  useLang()
  useAuthGuard()
  useAppearanceSync()
  const isMobile = useIsMobile()
  const { pathname } = useLocation()
  useEffect(
    () =>
      startEvents(queryClient, {
        onTask: (t) => {
          if (t.kind === 'assistant') useAssistant.getState().dispatch({ type: 'progress', taskId: t.task_id, done: t.done, total: t.total })
          else useToasts.getState().updateTask(t)
          genOnTask(queryClient, t)
        },
        onAssistant: (e) => void onAssistantDone(queryClient, e),
        onXmpConflict: (e) => handleXmpConflict(queryClient, e),
        onGen: (e) => genOnDone(queryClient, e),
        onStatus: (s) => useUi.getState().setConnection(s),
      }),
    [],
  )
  return (
    <>
      <div className="flex h-full flex-col">
        <div className="relative min-h-0 flex-1">
          <Routes>
            <Route path="/" element={<Home />} />
            <Route path="/s/:sessionId" element={<Library />} />
            <Route path="/s/:sessionId/people" element={<People />} />
            <Route path="/s/:sessionId/edit/:photoId" element={<Edit />} />
            <Route path="/s/:sessionId/besttake/:burstId" element={<BestTake />} />
            <Route path="/taste" element={<Taste />} />
            <Route path="/settings" element={<Settings />} />
            <Route path="/login" element={<Login />} />
            <Route path="*" element={<Navigate to="/" replace />} />
          </Routes>
        </div>
        {isMobile && showsBottomNav(pathname) && <BottomNav />}
      </div>
      <Onboarding />
      <FaceConsentDialog />
      <CatalogRecovery />
      <Toasts />
    </>
  )
}

export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <Shell />
      </BrowserRouter>
    </QueryClientProvider>
  )
}
