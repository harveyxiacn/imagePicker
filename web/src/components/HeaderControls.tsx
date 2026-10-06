import { Languages, Monitor, Moon, Sun } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useMe, usePatchSettings } from '@/api/queries'
import { can } from '@/lib/auth'
import { useUi, type Theme } from '@/stores/ui'

const NEXT: Record<Theme, Theme> = { dark: 'light', light: 'system', system: 'dark' }

export function HeaderControls() {
  const { t } = useTranslation()
  const theme = useUi((s) => s.theme)
  const setTheme = useUi((s) => s.setTheme)
  const lang = useUi((s) => s.lang)
  const setLang = useUi((s) => s.setLang)
  const owner = can(useMe().data?.role ?? null, 'settings')
  const patch = usePatchSettings()
  const Icon = theme === 'dark' ? Moon : theme === 'light' ? Sun : Monitor
  return (
    <div className="flex items-center gap-1">
      <button
        className="btn btn-ghost btn-icon"
        aria-label={t('app.theme', { v: t(`app.theme_${theme}`) })}
        title={t('app.theme', { v: t(`app.theme_${theme}`) })}
        onClick={() => {
          setTheme(NEXT[theme])
          if (owner) patch.mutate({ theme: NEXT[theme] })
        }}
      >
        <Icon size={15} />
      </button>
      <button
        className="btn btn-ghost"
        aria-label={t('app.language')}
        title={t('app.language')}
        onClick={() => {
          const next = lang === 'zh-CN' ? 'en' : 'zh-CN'
          setLang(next)
          if (owner) patch.mutate({ language: next })
        }}
      >
        <Languages size={15} />
        {lang === 'zh-CN' ? '中' : 'EN'}
      </button>
    </div>
  )
}
