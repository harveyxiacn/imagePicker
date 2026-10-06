import { useQueryClient } from '@tanstack/react-query'
import { Loader2 } from 'lucide-react'
import { useEffect, useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useParams } from 'react-router-dom'
import { useSession, usePhotos } from '@/api/queries'
import { Compare } from '@/components/Compare'
import { ExportDialog } from '@/components/ExportDialog'
import { FilterBar } from '@/components/FilterBar'
import { Grid } from '@/components/Grid'
import { HelpOverlay } from '@/components/HelpOverlay'
import { Inspector } from '@/components/Inspector'
import { Loupe } from '@/components/Loupe'
import { StatusBar } from '@/components/StatusBar'
import { TopBar } from '@/components/TopBar'
import { editPhotos } from '@/lib/actions'
import { useHistory } from '@/lib/history'
import { pruneSelection } from '@/lib/selection'
import { useUi } from '@/stores/ui'
import { compareSlots, useController } from './useController'
import { useShortcuts } from './useShortcuts'

export function Library() {
  const { sessionId: raw } = useParams()
  const sessionId = Number(raw)
  const { t } = useTranslation()
  const qc = useQueryClient()

  const filter = useUi((s) => s.filter)
  const view = useUi((s) => s.view)
  const activeId = useUi((s) => s.activeId)
  const selection = useUi((s) => s.selection)
  const compareA = useUi((s) => s.compareA)
  const compareCount = useUi((s) => s.compareCount)
  const syncZoom = useUi((s) => s.syncZoom)
  const inspectorOpen = useUi((s) => s.inspectorOpen)
  const exportOpen = useUi((s) => s.exportOpen)

  const session = useSession(sessionId)
  const photosQ = usePhotos(sessionId, filter)
  const photos = useMemo(() => photosQ.data?.photos ?? [], [photosQ.data])

  // Fresh transient state for each session.
  useEffect(() => {
    useUi.getState().resetSessionState()
    useHistory.getState().clear()
  }, [sessionId])

  const ctrl = useController(photos)
  useShortcuts(ctrl)

  // Keep the cursor valid as the list changes (filters, live updates).
  useEffect(() => {
    if (!photos.length) return
    const ui = useUi.getState()
    if (ui.activeId === null || !photos.some((p) => p.id === ui.activeId)) ui.setActive(photos[0].id)
    const ids = photos.map((p) => p.id)
    const pruned = pruneSelection(ui.selection, ids)
    if (pruned !== ui.selection) ui.setSelection(pruned)
  }, [photos])

  const activeIndex = activeId === null ? -1 : photos.findIndex((p) => p.id === activeId)
  const active = activeIndex >= 0 ? photos[activeIndex] : undefined
  const slots = useMemo(() => compareSlots(photos, compareA, activeId, compareCount), [photos, compareA, activeId, compareCount])

  const selectedIds = useMemo(() => [...selection.ids], [selection])
  const allIds = useMemo(() => photos.map((p) => p.id), [photos])
  const targetCount = view === 'grid' && selection.ids.size > 1 ? selection.ids.size : 1

  const pick = (id: number) => {
    const ui = useUi.getState()
    if (ui.view === 'compare' && id === ui.compareA) return
    ui.setActive(id)
  }

  if (session.isError && !session.data) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3">
        <div className="text-danger">{t('library.notFound')}</div>
        <Link to="/" className="btn">
          {t('common.back')}
        </Link>
      </div>
    )
  }

  return (
    <div className="flex h-full flex-col">
      <TopBar session={session.data} onView={ctrl.setView} />
      <FilterBar shown={photosQ.data?.total ?? 0} total={session.data?.photo_count ?? photosQ.data?.total ?? 0} showSize={view === 'grid'} />

      <div className="relative flex min-h-0 flex-1">
        <main className="min-w-0 flex-1">
          {photosQ.isPending ? (
            <div className="flex h-full items-center justify-center gap-2 text-muted">
              <Loader2 className="animate-spin" size={18} />
              {t('library.loading')}
            </div>
          ) : photosQ.isError ? (
            <div className="flex h-full flex-col items-center justify-center gap-3 text-danger">
              {(photosQ.error as Error).message}
              <button className="btn" onClick={() => void photosQ.refetch()}>
                {t('common.retry')}
              </button>
            </div>
          ) : view === 'grid' ? (
            <Grid photos={photos} onOpen={(id) => {
              useUi.getState().setActive(id)
              ctrl.setView('loupe')
            }} />
          ) : view === 'loupe' ? (
            <Loupe
              photos={photos}
              photo={active}
              index={activeIndex}
              onMove={(d) => ctrl.move(d)}
              onPick={pick}
              onRate={(n) => ctrl.rate(n ?? 0)}
            />
          ) : (
            <Compare
              photos={photos}
              slots={slots}
              activeId={activeId}
              compareA={compareA}
              count={compareCount}
              sync={syncZoom}
              onSyncChange={useUi.getState().setSyncZoom}
              onCountChange={useUi.getState().setCompareCount}
              onPick={pick}
              onSwap={ctrl.swapCompare}
              onRate={(id, n) => void editPhotos(qc, [id], { user_rating: n }, t('history.rating'))}
            />
          )}
        </main>

        {inspectorOpen && (
          <aside
            className="w-[300px] shrink-0 overflow-y-auto border-l border-line bg-panel max-lg:absolute max-lg:top-0 max-lg:right-0 max-lg:bottom-0 max-lg:z-30 max-lg:shadow-[var(--shadow)]"
            aria-label={t('top.inspector')}
          >
            <Inspector
              photo={active}
              targetCount={targetCount}
              onRate={(n) => ctrl.rate(n ?? 0)}
              onFlag={ctrl.setFlag}
              onColor={ctrl.setColor}
            />
          </aside>
        )}
      </div>

      <StatusBar session={session.data} photos={photos} />
      <HelpOverlay />
      <ExportDialog open={exportOpen} onOpenChange={useUi.getState().setExportOpen} selectedIds={selectedIds} allIds={allIds} />
    </div>
  )
}
