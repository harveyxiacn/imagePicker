import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import type { ConnectionStatus } from '@/api/events'
import { DEFAULT_FILTER, type FilterState } from '@/lib/filter'
import type { GridRow } from '@/lib/stacks'
import { EMPTY_SELECTION, type SelectionState } from '@/lib/selection'

export type View = 'grid' | 'loupe' | 'compare' | 'group'
export type Theme = 'dark' | 'light' | 'system'
export type Lang = 'zh-CN' | 'en'

interface UiState {
  view: View
  activeId: number | null
  selection: SelectionState
  /** Compare mode: pinned "A" photo (the candidate "B" is `activeId`). */
  compareA: number | null
  compareCount: 2 | 4
  syncZoom: boolean
  filter: FilterState
  /** Filter to apply when the next library mounts (e.g. coming from the People page). */
  pendingFilter: Partial<FilterState> | null
  /** Group view (B): burst being reviewed. */
  groupBurstId: number | null
  /** Stacks: expanded bursts / all, collapsed scene headers. */
  expandAll: boolean
  expandedStacks: ReadonlySet<number>
  collapsedScenes: ReadonlySet<number>
  /** Rows currently laid out by the grid (for row-aware arrow navigation). */
  gridRows: GridRow[]
  showFaces: boolean
  personFilterOpen: boolean
  acceptAllOpen: boolean
  helpOpen: boolean
  exportOpen: boolean
  /** assistant `export` plans preselect a preset (original / wechat / xiaohongshu / instagram) */
  exportPreset: string | null
  /** M4: save-as-smart-collection dialog, face search dialog */
  saveCollectionOpen: boolean
  faceSearchOpen: boolean
  /** Library left sidebar (smart collections) */
  sidebarOpen: boolean
  /** Mobile cull: one photo at a time, or quick cull (AI top-3 cards per group). */
  cullMode: 'single' | 'quick'
  connection: ConnectionStatus
  /** Increments when Space asks panes to toggle fit <-> 100%. */
  zoomToggle: number
  /** Grid column count (published by the Grid for ↑/↓ navigation). */
  gridCols: number
  setGridCols: (n: number) => void

  // persisted preferences
  thumbSize: number
  inspectorOpen: boolean
  theme: Theme
  lang: Lang
  /** Library shows scene headers + collapsed stacks (true) or a flat list (false). */
  grouped: boolean

  setGroupBurst: (id: number | null) => void
  setStacks: (s: { expandAll: boolean; expanded: ReadonlySet<number> }) => void
  setCollapsedScenes: (s: ReadonlySet<number>) => void
  setGridRows: (r: GridRow[]) => void
  setShowFaces: (b: boolean) => void
  setPersonFilterOpen: (b: boolean) => void
  setAcceptAllOpen: (b: boolean) => void
  setGrouped: (b: boolean) => void
  openWithFilter: (f: Partial<FilterState>) => void
  setView: (v: View) => void
  setActive: (id: number | null) => void
  setSelection: (s: SelectionState) => void
  setCompareA: (id: number | null) => void
  setCompareCount: (n: 2 | 4) => void
  setSyncZoom: (b: boolean) => void
  setFilter: (f: Partial<FilterState>) => void
  resetFilter: () => void
  setHelpOpen: (b: boolean) => void
  setExportOpen: (b: boolean) => void
  setExportPreset: (p: string | null) => void
  setSaveCollectionOpen: (b: boolean) => void
  setFaceSearchOpen: (b: boolean) => void
  setSidebarOpen: (b: boolean) => void
  setCullMode: (m: 'single' | 'quick') => void
  setConnection: (c: ConnectionStatus) => void
  toggleZoom: () => void
  setThumbSize: (n: number) => void
  setInspectorOpen: (b: boolean) => void
  setTheme: (t: Theme) => void
  setLang: (l: Lang) => void
  /** Reset per-session transient state when entering a library. */
  resetSessionState: () => void
}

const narrow = typeof window !== 'undefined' && window.matchMedia?.('(max-width: 1023px)').matches

export const useUi = create<UiState>()(
  persist(
    (set) => ({
      view: 'grid',
      activeId: null,
      selection: EMPTY_SELECTION,
      compareA: null,
      compareCount: 2,
      syncZoom: true,
      filter: DEFAULT_FILTER,
      pendingFilter: null,
      groupBurstId: null,
      expandAll: false,
      expandedStacks: new Set<number>(),
      collapsedScenes: new Set<number>(),
      gridRows: [],
      showFaces: false,
      personFilterOpen: false,
      acceptAllOpen: false,
      grouped: true,
      helpOpen: false,
      exportOpen: false,
      exportPreset: null,
      saveCollectionOpen: false,
      faceSearchOpen: false,
      sidebarOpen: !narrow,
      cullMode: 'single',
      connection: 'connecting',
      zoomToggle: 0,
      gridCols: 6,
      setGridCols: (gridCols) => set({ gridCols }),
      thumbSize: 176,
      inspectorOpen: !narrow,
      theme: 'dark',
      lang: 'zh-CN',

      setGroupBurst: (groupBurstId) => set({ groupBurstId }),
      setStacks: ({ expandAll, expanded }) => set({ expandAll, expandedStacks: expanded }),
      setCollapsedScenes: (collapsedScenes) => set({ collapsedScenes }),
      setGridRows: (gridRows) => set({ gridRows }),
      setShowFaces: (showFaces) => set({ showFaces }),
      setPersonFilterOpen: (personFilterOpen) => set({ personFilterOpen }),
      setAcceptAllOpen: (acceptAllOpen) => set({ acceptAllOpen }),
      setGrouped: (grouped) => set({ grouped }),
      openWithFilter: (f) => set({ pendingFilter: f }),
      setView: (view) => set({ view }),
      setActive: (activeId) => set({ activeId }),
      setSelection: (selection) => set({ selection }),
      setCompareA: (compareA) => set({ compareA }),
      setCompareCount: (compareCount) => set({ compareCount }),
      setSyncZoom: (syncZoom) => set({ syncZoom }),
      setFilter: (f) => set((s) => ({ filter: { ...s.filter, ...f } })),
      resetFilter: () => set((s) => ({ filter: { ...DEFAULT_FILTER, sort: s.filter.sort } })),
      setHelpOpen: (helpOpen) => set({ helpOpen }),
      setExportOpen: (exportOpen) => set({ exportOpen }),
      setExportPreset: (exportPreset) => set({ exportPreset }),
      setSaveCollectionOpen: (saveCollectionOpen) => set({ saveCollectionOpen }),
      setFaceSearchOpen: (faceSearchOpen) => set({ faceSearchOpen }),
      setSidebarOpen: (sidebarOpen) => set({ sidebarOpen }),
      setCullMode: (cullMode) => set({ cullMode }),
      setConnection: (connection) => set({ connection }),
      toggleZoom: () => set((s) => ({ zoomToggle: s.zoomToggle + 1 })),
      setThumbSize: (thumbSize) => set({ thumbSize }),
      setInspectorOpen: (inspectorOpen) => set({ inspectorOpen }),
      setTheme: (theme) => set({ theme }),
      setLang: (lang) => set({ lang }),
      resetSessionState: () =>
        set((s) => ({
          view: 'grid',
          activeId: null,
          selection: EMPTY_SELECTION,
          compareA: null,
          filter: { ...DEFAULT_FILTER, ...s.pendingFilter },
          pendingFilter: null,
          groupBurstId: null,
          expandAll: false,
          expandedStacks: new Set<number>(),
          collapsedScenes: new Set<number>(),
          gridRows: [],
          personFilterOpen: false,
          acceptAllOpen: false,
          helpOpen: false,
          exportOpen: false,
          saveCollectionOpen: false,
          faceSearchOpen: false,
        })),
    }),
    {
      name: 'imagepicker.ui',
      partialize: (s) => ({
        thumbSize: s.thumbSize,
        inspectorOpen: s.inspectorOpen,
        sidebarOpen: s.sidebarOpen,
        theme: s.theme,
        lang: s.lang,
        syncZoom: s.syncZoom,
        compareCount: s.compareCount,
        grouped: s.grouped,
        showFaces: s.showFaces,
      }),
    },
  ),
)
