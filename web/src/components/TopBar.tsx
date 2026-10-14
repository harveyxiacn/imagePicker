import { ChevronLeft, Columns2, Download, GalleryHorizontal, HelpCircle, LayoutGrid, PanelLeft, PanelRight, Settings as SettingsIcon, Square, Users } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'
import { useMe } from '@/api/queries'
import type { Session } from '@/api/types'
import { can } from '@/lib/auth'
import { useUi, type View } from '@/stores/ui'
import { AnalyzeControl, WorkerChip } from './AnalyzeControl'
import { AssistantButton } from './assistant/AssistantDrawer'
import { HeaderControls } from './HeaderControls'
import { SearchBox } from './SearchBox'
import { UserMenu } from './UserMenu'

const VIEWS: { v: View; icon: typeof LayoutGrid; key: string }[] = [
  { v: 'grid', icon: LayoutGrid, key: 'G' },
  { v: 'loupe', icon: Square, key: 'E' },
  { v: 'compare', icon: Columns2, key: 'C' },
  { v: 'group', icon: GalleryHorizontal, key: 'B' },
]

interface Props {
  sessionId: number
  session: Session | undefined
  onView: (v: View) => void
}

export function TopBar({ sessionId, session, onView }: Props) {
  const { t } = useTranslation()
  const view = useUi((s) => s.view)
  const inspectorOpen = useUi((s) => s.inspectorOpen)
  const setInspectorOpen = useUi((s) => s.setInspectorOpen)
  const setHelpOpen = useUi((s) => s.setHelpOpen)
  const setExportOpen = useUi((s) => s.setExportOpen)
  const sidebarOpen = useUi((s) => s.sidebarOpen)
  const setSidebarOpen = useUi((s) => s.setSidebarOpen)
  const role = useMe().data?.role ?? 'owner'

  return (
    <header className="flex h-12 shrink-0 items-center gap-2 border-b border-line px-2 sm:gap-3 sm:px-3">
      <button
        className="btn btn-ghost btn-icon"
        aria-pressed={sidebarOpen}
        aria-label={t('collections.toggle')}
        title={t('collections.toggle')}
        onClick={() => setSidebarOpen(!sidebarOpen)}
        data-testid="sidebar-toggle"
      >
        <PanelLeft size={16} />
      </button>
      <Link to="/" className="btn btn-ghost" aria-label={t('common.back')} title={t('common.back')}>
        <ChevronLeft size={16} />
        <span className="hidden max-w-48 truncate font-semibold sm:inline">{session?.title ?? '…'}</span>
      </Link>

      <div className="flex rounded-control border border-line p-0.5" role="tablist" aria-label={t('view.label')}>
        {VIEWS.map(({ v, icon: Icon, key }) => (
          <button
            key={v}
            role="tab"
            aria-selected={view === v}
            className={`flex h-6 items-center gap-1.5 rounded px-2 transition-colors ${
              view === v ? 'bg-accent text-accent-fg' : 'text-muted hover:text-fg'
            }`}
            title={`${t(`view.${v}`)} (${key})`}
            onClick={() => onView(v)}
          >
            <Icon size={14} />
            <span className="hidden md:inline">{t(`view.${v}`)}</span>
          </button>
        ))}
      </div>

      <SearchBox sessionId={sessionId} />

      <div className="ml-auto flex items-center gap-1 sm:gap-2">
        <WorkerChip />
        {can(role, 'analyze') && (
          <div data-coach="analyze">
            <AnalyzeControl sessionId={sessionId} photoCount={session?.photo_count ?? 0} />
          </div>
        )}
        <AssistantButton />
        {can(role, 'people') && (
          <Link to={`/s/${sessionId}/people`} className="btn" title={t('people.title')} data-testid="people-link">
            <Users size={14} />
            <span className="hidden lg:inline">{t('people.title')}</span>
          </Link>
        )}
        {can(role, 'export') && (
          <button className="btn" onClick={() => setExportOpen(true)} title={`${t('top.export')} (Ctrl+E)`}>
            <Download size={14} />
            <span className="hidden sm:inline">{t('top.export')}</span>
          </button>
        )}
        <HeaderControls />
        {can(role, 'settings') && (
          <Link to="/settings" className="btn btn-ghost btn-icon" aria-label={t('home.settings')} title={t('home.settings')} data-testid="settings-link">
            <SettingsIcon size={16} />
          </Link>
        )}
        <UserMenu />
        <button className="btn btn-ghost btn-icon" onClick={() => setHelpOpen(true)} aria-label={t('top.help')} title={`${t('top.help')} (?)`}>
          <HelpCircle size={16} />
        </button>
        <button
          className="btn btn-ghost btn-icon"
          aria-pressed={inspectorOpen}
          aria-label={t('top.inspector')}
          title={`${t('top.inspector')} (Tab)`}
          onClick={() => setInspectorOpen(!inspectorOpen)}
        >
          <PanelRight size={16} />
        </button>
      </div>
    </header>
  )
}
