import { useEffect } from 'react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom'
import { startEvents } from '@/api/events'
import { Home } from '@/pages/Home'
import { Edit } from '@/pages/Edit'
import { Library } from '@/pages/Library'
import { People } from '@/pages/People'
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

function Shell() {
  useTheme()
  useLang()
  useEffect(
    () =>
      startEvents(queryClient, {
        onTask: (t) => useToasts.getState().updateTask(t),
        onStatus: (s) => useUi.getState().setConnection(s),
      }),
    [],
  )
  return (
    <>
      <Routes>
        <Route path="/" element={<Home />} />
        <Route path="/s/:sessionId" element={<Library />} />
        <Route path="/s/:sessionId/people" element={<People />} />
        <Route path="/s/:sessionId/edit/:photoId" element={<Edit />} />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
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
