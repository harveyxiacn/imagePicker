import { useEffect } from 'react'
import { COLOR_KEYS, dispatchKey, isTypingTarget, type Handlers, type Scope } from '@/lib/keymap'
import { useUi } from '@/stores/ui'
import type { Controller } from './useController'

const REPEATABLE = new Set(['nav.prev', 'nav.next', 'nav.up', 'nav.down', 'edit.undo', 'edit.redo'])

export function buildHandlers(ctrl: Controller): Handlers {
  const ui = () => useUi.getState()
  const h: Handlers = {}
  for (let n = 0; n <= 5; n++) {
    h[`rate.${n}`] = () => ctrl.rate(n)
    h[`rateNext.${n}`] = () => ctrl.rate(n, true)
  }
  h['flag.pick'] = () => ctrl.setFlag(1)
  h['flag.reject'] = () => ctrl.setFlag(-1)
  h['flag.none'] = () => ctrl.setFlag(0)
  for (const c of Object.values(COLOR_KEYS)) h[`color.${c}`] = () => ctrl.setColor(c)
  h['nav.prev'] = () => ctrl.move(-1)
  h['nav.next'] = () => ctrl.move(1)
  h['nav.up'] = () => ctrl.moveRow(-1)
  h['nav.down'] = () => ctrl.moveRow(1)
  h['nav.first'] = () => ctrl.setActiveIndex(0)
  h['nav.last'] = () => ctrl.setActiveIndex(Number.MAX_SAFE_INTEGER)
  h['nav.open'] = () => ctrl.setView('loupe')
  h['zoom.open'] = () => ctrl.setView('loupe')
  h['nav.back'] = () => {
    const s = ui()
    if (s.view !== 'grid') ctrl.setView('grid')
    else if (s.selection.ids.size) s.setSelection({ ids: new Set(), anchorId: null })
  }
  h['view.grid'] = () => ctrl.setView('grid')
  h['view.loupe'] = () => ctrl.setView('loupe')
  h['view.compare'] = () => ctrl.setView('compare')
  h['view.group'] = () => ctrl.setView('group')
  h['ai.accept'] = () => void ctrl.acceptAi()
  h['ai.acceptAll'] = () => ui().setAcceptAllOpen(true)
  h['stacks.toggle'] = () => ctrl.stacks(false)
  h['stacks.toggleAll'] = () => ctrl.stacks(true)
  h['group.prev'] = () => ctrl.jump(-1)
  h['group.next'] = () => ctrl.jump(1)
  h['group.pick'] = () => ctrl.groupPick(false)
  h['group.pickNext'] = () => ctrl.groupPick(true)
  h['faces.toggle'] = () => ui().setShowFaces(!ui().showFaces)
  h['personFilter.open'] = () => ui().setPersonFilterOpen(!ui().personFilterOpen)
  h['view.fullscreen'] = () => {
    if (document.fullscreenElement) void document.exitFullscreen()
    else void document.documentElement.requestFullscreen?.().catch(() => undefined)
  }
  h['view.sidebar'] = () => ui().setInspectorOpen(!ui().inspectorOpen)
  h['compare.swap'] = () => ctrl.swapCompare()
  h['compare.sync'] = () => ui().setSyncZoom(!ui().syncZoom)
  h['zoom.toggle'] = () => ui().toggleZoom()
  h['edit.undo'] = () => void ctrl.undo()
  h['edit.redo'] = () => void ctrl.redo()
  h['select.all'] = () => ctrl.selectAll()
  h['edit.open'] = () => ctrl.openEdit()
  h['export.open'] = () => ui().setExportOpen(true)
  h['help.toggle'] = () => ui().setHelpOpen(!ui().helpOpen)
  return h
}

/** Window-level keyboard handler for the library, driven by the central keymap. */
export function useShortcuts(ctrl: Controller) {
  useEffect(() => {
    const handlers = buildHandlers(ctrl)
    const onKey = (e: KeyboardEvent) => {
      if (isTypingTarget(e.target)) return
      const ui = useUi.getState()
      // While a dialog is open only the help toggle is active (Radix handles Escape).
      const dialogOpen = !!document.querySelector('[role="dialog"]')
      if (dialogOpen && e.key !== '?') return
      const scope: Scope = ui.view

      // Shift+arrows extend the selection in the grid.
      if (scope === 'grid' && e.shiftKey && !e.ctrlKey && !e.metaKey) {
        const d = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -ui.gridCols, ArrowDown: ui.gridCols }[e.key]
        if (d) {
          e.preventDefault()
          ctrl.move(d, { extend: true })
          return
        }
      }

      const handled = dispatchKey(e, scope, new Proxy(handlers, {
        get: (target, id: string) => {
          const fn = target[id]
          if (!fn) return undefined
          // Suppress OS key-repeat for one-shot actions.
          return e.repeat && !REPEATABLE.has(id) ? () => undefined : fn
        },
      }))
      if (handled) e.preventDefault()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [ctrl])
}
