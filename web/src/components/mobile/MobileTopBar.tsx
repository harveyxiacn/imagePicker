import { ChevronLeft, Columns2, Download, FolderTree, GalleryHorizontal, HelpCircle, Info, LayoutGrid, MoreVertical, SlidersHorizontal, Square, Users } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'
import { useMe } from '@/api/queries'
import type { FlagFilter, Session } from '@/api/types'
import { can } from '@/lib/auth'
import { isFilterActive } from '@/lib/filter'
import { useUi, type View } from '@/stores/ui'
import { AnalyzeControl } from '../AnalyzeControl'
import { AssistantButton } from '../assistant/AssistantDrawer'
import { FilterBar } from '../FilterBar'
import { HeaderControls } from '../HeaderControls'
import { Sheet } from './Sheet'

const VIEWS: { v: View; icon: typeof LayoutGrid }[] = [
  { v: 'grid', icon: LayoutGrid },
  { v: 'loupe', icon: Square },
  { v: 'compare', icon: Columns2 },
  { v: 'group', icon: GalleryHorizontal },
]

const FLAG_CHIPS: ('all' | FlagFilter)[] = ['all', 'picked', 'unflagged', 'rejected']

interface Props {
  sessionId: number
  session: Session | undefined
  shown: number
  total: number
  onView: (v: View) => void
}

/** Phone header: back + title + filter sheet + analyze + overflow menu, with quick flag chips underneath. */
export function MobileTopBar({ sessionId, session, shown, total, onView }: Props) {
  const { t } = useTranslation()
  const filter = useUi((s) => s.filter)
  const setFilter = useUi((s) => s.setFilter)
  const view = useUi((s) => s.view)
  const setSidebarOpen = useUi((s) => s.setSidebarOpen)
  const setExportOpen = useUi((s) => s.setExportOpen)
  const setHelpOpen = useUi((s) => s.setHelpOpen)
  const setInspectorOpen = useUi((s) => s.setInspectorOpen)
  const role = useMe().data?.role ?? 'owner'
  const [filterOpen, setFilterOpen] = useState(false)
  const [menuOpen, setMenuOpen] = useState(false)
  const active = isFilterActive(filter)

  const item = 'flex min-h-12 w-full items-center gap-3 px-4 text-left text-base active:bg-elevated'

  return (
    <header className="shrink-0 border-b border-line" data-testid="mobile-topbar">
      <div className="flex h-12 items-center gap-1 px-1">
        <Link to="/" className="btn btn-ghost btn-icon" aria-label={t('common.back')}>
          <ChevronLeft size={20} />
        </Link>
        <div className="min-w-0 flex-1">
          <div className="truncate text-base leading-tight font-semibold">{session?.title ?? '…'}</div>
          <div className="tnum text-[11px] leading-tight text-muted">{shown === total ? total.toLocaleString() : `${shown.toLocaleString()} / ${total.toLocaleString()}`}</div>
        </div>
        <button className="btn btn-ghost btn-icon relative" aria-label={t('mobile.filter')} onClick={() => setFilterOpen(true)} data-testid="mobile-filter">
          <SlidersHorizontal size={18} />
          {active && <span className="absolute top-2 right-2 h-2 w-2 rounded-full bg-accent" />}
        </button>
        {can(role, 'analyze') && <AnalyzeControl sessionId={sessionId} photoCount={session?.photo_count ?? 0} />}
        <button className="btn btn-ghost btn-icon" aria-label={t('mobile.more')} onClick={() => setMenuOpen(true)} data-testid="mobile-more">
          <MoreVertical size={18} />
        </button>
      </div>
      <div className="flex gap-1.5 overflow-x-auto px-2 pb-2" role="group" aria-label={t('filter.flag')} data-testid="flag-chips">
        {FLAG_CHIPS.map((f) => (
          <button key={f} className="chip !h-11 !px-4 !text-sm" aria-pressed={filter.flag === f} onClick={() => setFilter({ flag: f })} data-testid={`flag-chip-${f}`}>
            {t(`filter.flag_${f}`)}
          </button>
        ))}
      </div>

      <Sheet open={filterOpen} onOpenChange={setFilterOpen} title={t('mobile.filter')} testId="filter-sheet">
        <FilterBar sessionId={sessionId} shown={shown} total={total} showSize={view === 'grid'} />
      </Sheet>

      <Sheet open={menuOpen} onOpenChange={setMenuOpen} title={t('mobile.more')} testId="more-sheet">
        <div className="flex flex-col py-1">
          <div className="flex gap-1.5 px-4 py-2" role="tablist" aria-label={t('view.label')}>
            {VIEWS.map(({ v, icon: Icon }) => (
              <button
                key={v}
                role="tab"
                aria-selected={view === v}
                className={`flex min-h-12 flex-1 flex-col items-center justify-center gap-0.5 rounded-control border text-xs ${view === v ? 'border-accent bg-accent/15' : 'border-line'}`}
                onClick={() => {
                  setMenuOpen(false)
                  onView(v)
                }}
              >
                <Icon size={18} />
                {t(`view.${v}`)}
              </button>
            ))}
          </div>
          <button className={item} onClick={() => { setMenuOpen(false); setSidebarOpen(true) }}>
            <FolderTree size={18} className="text-muted" />
            {t('collections.toggle')}
          </button>
          {can(role, 'people') && (
            <Link to={`/s/${sessionId}/people`} className={item} onClick={() => setMenuOpen(false)}>
              <Users size={18} className="text-muted" />
              {t('people.title')}
            </Link>
          )}
          {can(role, 'export') && (
            <button className={item} onClick={() => { setMenuOpen(false); setExportOpen(true) }}>
              <Download size={18} className="text-muted" />
              {t('top.export')}
            </button>
          )}
          <button className={item} onClick={() => { setMenuOpen(false); setInspectorOpen(true) }}>
            <Info size={18} className="text-muted" />
            {t('top.inspector')}
          </button>
          <button className={item} onClick={() => { setMenuOpen(false); setHelpOpen(true) }}>
            <HelpCircle size={18} className="text-muted" />
            {t('top.help')}
          </button>
          <div className="flex items-center gap-3 px-4 py-2">
            <HeaderControls />
            <AssistantButton />
          </div>
        </div>
      </Sheet>
    </header>
  )
}
