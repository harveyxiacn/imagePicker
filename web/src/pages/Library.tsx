import { useQueryClient } from '@tanstack/react-query'
import { Loader2 } from 'lucide-react'
import { useEffect, useMemo, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useLocation, useNavigate, useParams } from 'react-router-dom'
import { useGroups, usePhotos, useSession } from '@/api/queries'
import { AssistantHost } from '@/components/assistant/AssistantDrawer'
import { CollectionsSidebar } from '@/components/CollectionsSidebar'
import { Compare } from '@/components/Compare'
import { ExportDialog } from '@/components/ExportDialog'
import { FaceSearchDialog } from '@/components/FaceSearchDialog'
import { FilterBar } from '@/components/FilterBar'
import { Grid } from '@/components/Grid'
import { GroupView } from '@/components/GroupView'
import { HelpOverlay } from '@/components/HelpOverlay'
import { Inspector } from '@/components/Inspector'
import { Loupe } from '@/components/Loupe'
import { CullView } from '@/components/mobile/CullView'
import { MobileTopBar } from '@/components/mobile/MobileTopBar'
import { QuickCull } from '@/components/mobile/QuickCull'
import { Sheet } from '@/components/mobile/Sheet'
import { Modal } from '@/components/Modal'
import { ModelConsentDialog } from '@/components/ModelConsentDialog'
import { usePhotoMenu } from '@/components/PhotoMenu'
import { SaveCollectionDialog } from '@/components/SaveCollectionDialog'
import { StatusBar } from '@/components/StatusBar'
import { TopBar } from '@/components/TopBar'
import { acceptAiRatings, editPhotos } from '@/lib/actions'
import { canAccept } from '@/lib/ai'
import { useHistory } from '@/lib/history'
import { pruneSelection } from '@/lib/selection'
import { useIsMobile } from '@/lib/useLayout'
import { buildGridItems, toggleInSet, visiblePhotos } from '@/lib/stacks'
import { useToasts } from '@/stores/toasts'
import { useUi, type View } from '@/stores/ui'
import { compareSlots, useController } from './useController'
import { useGroup } from './useGroup'
import { useShortcuts } from './useShortcuts'

export function Library() {
  const { sessionId: raw } = useParams()
  const sessionId = Number(raw)
  const { t } = useTranslation()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const location = useLocation()
  const keepState = useRef(!!(location.state as { keep?: boolean } | null)?.keep)
  const startView = useRef((location.state as { view?: View } | null)?.view)
  const isMobile = useIsMobile()
  const cullMode = useUi((s) => s.cullMode)

  const filter = useUi((s) => s.filter)
  const view = useUi((s) => s.view)
  const activeId = useUi((s) => s.activeId)
  const selection = useUi((s) => s.selection)
  const compareA = useUi((s) => s.compareA)
  const compareCount = useUi((s) => s.compareCount)
  const syncZoom = useUi((s) => s.syncZoom)
  const inspectorOpen = useUi((s) => s.inspectorOpen)
  const showFaces = useUi((s) => s.showFaces)
  const sidebarOpen = useUi((s) => s.sidebarOpen)
  const photoMenu = usePhotoMenu(sessionId)
  const exportOpen = useUi((s) => s.exportOpen)
  const grouped = useUi((s) => s.grouped)
  const expandAll = useUi((s) => s.expandAll)
  const expandedStacks = useUi((s) => s.expandedStacks)
  const collapsedScenes = useUi((s) => s.collapsedScenes)
  const acceptAllOpen = useUi((s) => s.acceptAllOpen)

  const session = useSession(sessionId)
  const photosQ = usePhotos(sessionId, filter)
  const photos = useMemo(() => photosQ.data?.photos ?? [], [photosQ.data])
  const scenesQ = useGroups(sessionId)

  // Fresh transient state for each session.
  // (The ref makes this idempotent under StrictMode's double effect, which would otherwise discard a pending filter.)
  const resetFor = useRef<number | null>(null)
  useEffect(() => {
    if (resetFor.current === sessionId) return
    resetFor.current = sessionId
    // Coming back from the edit page: keep filter / cursor / undo history.
    if (keepState.current) return
    useUi.getState().resetSessionState()
    useHistory.getState().clear()
    // Bottom-nav "cull" tab from another page: open straight in the cull view.
    if (startView.current) useUi.getState().setView(startView.current)
  }, [sessionId])

  // Grouped mode: scene headers (time order only) + bursts collapsed to their best shot.
  const withHeaders = filter.sort === 'taken_at' || filter.sort === '-taken_at'
  const items = useMemo(
    () => buildGridItems(photos, { grouped, expandAll, expanded: expandedStacks, collapsedScenes, scenes: scenesQ.data, withHeaders }),
    [photos, grouped, expandAll, expandedStacks, collapsedScenes, scenesQ.data, withHeaders],
  )
  const gridPhotos = useMemo(() => visiblePhotos(items), [items])

  const group = useGroup(sessionId)
  // Navigation / actions operate on what the current view shows.
  const listPhotos = view === 'group' ? group.photos : gridPhotos
  const openEdit = () => {
    const id = useUi.getState().activeId
    if (id !== null) navigate(`/s/${sessionId}/edit/${id}`)
  }
  const ctrl = useController(listPhotos, { groupGo: group.go, groupPick: group.pickA, openEdit })
  useShortcuts(ctrl)

  // Keep the cursor valid as the list changes (filters, live updates, collapsed stacks).
  useEffect(() => {
    if (!listPhotos.length) return
    const ui = useUi.getState()
    if (ui.activeId === null || !listPhotos.some((p) => p.id === ui.activeId)) {
      // A photo hidden inside a collapsed stack: move to the stack's cover.
      const hidden = photos.find((p) => p.id === ui.activeId)
      const cover = hidden?.burst_id != null ? listPhotos.find((p) => p.burst_id === hidden.burst_id) : undefined
      ui.setActive((cover ?? listPhotos[0]).id)
    }
    const pruned = pruneSelection(ui.selection, gridPhotos.map((p) => p.id))
    if (pruned !== ui.selection) ui.setSelection(pruned)
  }, [listPhotos, gridPhotos, photos])

  const activeIndex = activeId === null ? -1 : listPhotos.findIndex((p) => p.id === activeId)
  const active = activeIndex >= 0 ? listPhotos[activeIndex] : undefined
  const slots = useMemo(() => compareSlots(listPhotos, compareA, activeId, compareCount), [listPhotos, compareA, activeId, compareCount])
  const groupSlots = useMemo(() => compareSlots(group.photos, compareA, activeId, 2), [group.photos, compareA, activeId])
  const acceptable = useMemo(() => photos.filter(canAccept).length, [photos])

  const selectedIds = useMemo(() => [...selection.ids], [selection])
  const allIds = useMemo(() => gridPhotos.map((p) => p.id), [gridPhotos])
  const targetCount = view === 'grid' && selection.ids.size > 1 ? selection.ids.size : 1

  const pick = (id: number) => {
    const ui = useUi.getState()
    if ((ui.view === 'compare' || ui.view === 'group') && id === ui.compareA) return
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
      {isMobile && view === 'loupe' ? null : isMobile ? (
        <MobileTopBar
          sessionId={sessionId}
          session={session.data}
          shown={photosQ.data?.total ?? 0}
          total={session.data?.photo_count ?? photosQ.data?.total ?? 0}
          onView={ctrl.setView}
        />
      ) : (
        <>
          <TopBar sessionId={sessionId} session={session.data} onView={ctrl.setView} />
          <FilterBar
            sessionId={sessionId}
            shown={photosQ.data?.total ?? 0}
            total={session.data?.photo_count ?? photosQ.data?.total ?? 0}
            showSize={view === 'grid'}
          />
        </>
      )}

      <div className="relative flex min-h-0 flex-1">
        {sidebarOpen && !isMobile && (
          <aside className="shrink-0 max-lg:absolute max-lg:top-0 max-lg:left-0 max-lg:bottom-0 max-lg:z-30 max-lg:shadow-[var(--shadow)]">
            <CollectionsSidebar sessionId={sessionId} />
          </aside>
        )}
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
            <Grid
              items={items}
              onContext={photoMenu.open}
              onOpen={(id) => {
                useUi.getState().setActive(id)
                ctrl.setView('loupe')
              }}
              onToggleStack={(id) => {
                const ui = useUi.getState()
                ui.setStacks(
                  ui.expandAll ? { expandAll: false, expanded: new Set() } : { expandAll: false, expanded: toggleInSet(ui.expandedStacks, id) },
                )
              }}
              onToggleScene={(id) => {
                const ui = useUi.getState()
                ui.setCollapsedScenes(toggleInSet(ui.collapsedScenes, id))
              }}
            />
          ) : view === 'loupe' && isMobile ? (
            cullMode === 'quick' ? (
              <QuickCull key={sessionId} photos={photos} onBack={() => useUi.getState().setCullMode('single')} />
            ) : (
              <CullView
                photos={listPhotos}
                photo={active}
                index={activeIndex}
                onMove={(d) => ctrl.move(d)}
                onFlag={ctrl.setFlag}
                onRate={(n) => ctrl.rate(n ?? 0)}
                onEdit={openEdit}
                onBack={() => ctrl.setView('grid')}
                onInfo={() => useUi.getState().setInspectorOpen(true)}
                onQuick={() => useUi.getState().setCullMode('quick')}
                showFaces={showFaces}
              />
            )
          ) : view === 'loupe' ? (
            <Loupe
              photos={listPhotos}
              photo={active}
              index={activeIndex}
              onMove={(d) => ctrl.move(d)}
              onPick={pick}
              onRate={(n) => ctrl.rate(n ?? 0)}
              onEdit={openEdit}
            />
          ) : view === 'group' ? (
            <GroupView
              group={group}
              slots={groupSlots}
              onPick={pick}
              onSwap={ctrl.swapCompare}
              onRate={(id, n) => void editPhotos(qc, [id], { user_rating: n }, t('history.rating'))}
            />
          ) : (
            <Compare
              photos={listPhotos}
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

        {inspectorOpen && !isMobile && (
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
              onAcceptAi={() => void ctrl.acceptAi()}
            />
          </aside>
        )}
      </div>

      {isMobile && (
        <>
          <Sheet open={inspectorOpen} onOpenChange={useUi.getState().setInspectorOpen} title={t('top.inspector')} testId="inspector-sheet">
            <Inspector
              photo={active}
              targetCount={targetCount}
              onRate={(n) => ctrl.rate(n ?? 0)}
              onFlag={ctrl.setFlag}
              onColor={ctrl.setColor}
              onAcceptAi={() => void ctrl.acceptAi()}
            />
          </Sheet>
          <Sheet open={sidebarOpen} onOpenChange={useUi.getState().setSidebarOpen} title={t('collections.toggle')} testId="collections-sheet">
            <div className="min-h-[40dvh] [&_nav]:!w-full [&_nav]:!border-r-0">
              <CollectionsSidebar sessionId={sessionId} />
            </div>
          </Sheet>
        </>
      )}
      {!isMobile && <StatusBar sessionId={sessionId} session={session.data} photos={photos} />}
      <HelpOverlay />
      <ModelConsentDialog />
      <AssistantHost sessionId={sessionId} />
      <Modal
        open={acceptAllOpen}
        onOpenChange={useUi.getState().setAcceptAllOpen}
        title={t('ai.acceptAllTitle')}
        description={t('ai.acceptAllDesc', { n: acceptable })}
        width="max-w-md"
        footer={
          <>
            <button className="btn" onClick={() => useUi.getState().setAcceptAllOpen(false)}>
              {t('common.cancel')}
            </button>
            <button
              className="btn btn-primary"
              disabled={acceptable === 0}
              data-testid="accept-all-confirm"
              onClick={() => {
                useUi.getState().setAcceptAllOpen(false)
                void acceptAiRatings(
                  qc,
                  photos.map((p) => p.id),
                  t('history.acceptAi'),
                ).then((n) => useToasts.getState().push('info', n > 0 ? t('ai.accepted', { n }) : t('ai.nothingToAccept'), 2500))
              }}
            >
              {t('ai.acceptAllConfirm', { n: acceptable })}
            </button>
          </>
        }
      >
        <div className="text-muted">{t('ai.acceptAllHint')}</div>
      </Modal>
      {photoMenu.node}
      <SaveCollectionDialog />
      <FaceSearchDialog sessionId={sessionId} />
      <ExportDialog open={exportOpen} onOpenChange={useUi.getState().setExportOpen} selectedIds={selectedIds} allIds={allIds} />
    </div>
  )
}
