import { Cpu, Database, FileJson, Globe, Keyboard, Palette, ScanFace, ScanSearch, Wifi, ChevronLeft, Layers } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Link, Navigate, useSearchParams } from 'react-router-dom'
import { useMe, useSettings } from '@/api/queries'
import { AppearanceSection, ShortcutsSection } from '@/components/settings/MiscSections'
import { HardwareSection } from '@/components/settings/HardwareSection'
import { LanSection, XmpSection } from '@/components/settings/LanXmpSections'
import { AnalysisSection, CacheSection, FacesSection, RenderSection } from '@/components/settings/PrivacyCacheSections'
import { can } from '@/lib/auth'

const SETTINGS_SECTIONS = [
  { id: 'hardware', icon: Cpu, view: HardwareSection },
  { id: 'analysis', icon: ScanSearch, view: AnalysisSection },
  { id: 'faces', icon: ScanFace, view: FacesSection },
  { id: 'cache', icon: Database, view: CacheSection },
  { id: 'render', icon: Layers, view: RenderSection },
  { id: 'lan', icon: Wifi, view: LanSection },
  { id: 'xmp', icon: FileJson, view: XmpSection },
  { id: 'keys', icon: Keyboard, view: ShortcutsSection },
  { id: 'appearance', icon: Palette, view: AppearanceSection },
] as const

/** Settings page (route `/settings`, doc 04 section 2). One section at a time; `?section=<id>` selects it. */
export function Settings() {
  const { t } = useTranslation()
  const me = useMe()
  const settings = useSettings(!!me.data && can(me.data.role, 'settings'))
  const [params, setParams] = useSearchParams()
  const current = SETTINGS_SECTIONS.find((s) => s.id === params.get('section')) ?? SETTINGS_SECTIONS[0]
  const View = current.view

  if (me.data && !can(me.data.role, 'settings')) return <Navigate to="/" replace />

  return (
    <div className="flex h-full flex-col" data-testid="settings-page">
      <header className="flex h-12 shrink-0 items-center gap-2 border-b border-line px-3">
        <Link to="/" className="btn btn-ghost" aria-label={t('common.back')} data-testid="settings-back">
          <ChevronLeft size={16} />
          <span className="hidden sm:inline">{t('common.back')}</span>
        </Link>
        <h1 className="flex items-center gap-2 text-base font-semibold">
          <Globe size={16} className="text-accent" />
          {t('settings.title')}
        </h1>
      </header>
      <div className="flex min-h-0 flex-1 max-md:flex-col">
        <nav className="shrink-0 border-line max-md:overflow-x-auto max-md:border-b md:w-56 md:overflow-y-auto md:border-r" aria-label={t('settings.title')}>
          <ul className="flex gap-0.5 p-2 md:flex-col">
            {SETTINGS_SECTIONS.map(({ id, icon: Icon }) => (
              <li key={id} className="shrink-0">
                <button
                  className={`flex w-full items-center gap-2 rounded-control px-3 py-1.5 text-left whitespace-nowrap transition-colors ${current.id === id ? 'bg-accent/15 font-medium text-fg' : 'text-muted hover:bg-elevated hover:text-fg'}`}
                  aria-current={current.id === id ? 'page' : undefined}
                  onClick={() => setParams({ section: id }, { replace: true })}
                  data-testid={`settings-nav-${id}`}
                >
                  <Icon size={15} />
                  {t(`settings.nav.${id}`)}
                </button>
              </li>
            ))}
          </ul>
        </nav>
        <main className="min-h-0 min-w-0 flex-1 overflow-y-auto">
          <div className="mx-auto flex max-w-3xl flex-col gap-4 p-4 md:p-6">
            <h2 className="text-xl font-semibold">{t(`settings.nav.${current.id}`)}</h2>
            {settings.isError ? <div className="text-danger">{(settings.error as Error).message}</div> : <View key={current.id} />}
          </div>
        </main>
      </div>
    </div>
  )
}
