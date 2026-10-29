import { useEffect, useId, useRef, type RefObject } from 'react'
import { rovingIndex, tabIndexIn } from './popover'

const FOCUSABLE = 'a[href],button:not([disabled]),input:not([disabled]):not([type="hidden"]),select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex="-1"])'
const MENU_ITEMS = '[role="menuitem"]:not([disabled]):not([aria-disabled="true"])'

interface Options {
  open: boolean
  onClose: () => void
  /** `role="menu"` popovers also get ArrowUp / ArrowDown / Home / End across their `role="menuitem"` children. */
  menu?: boolean
  /** The popover element (focus moves into it on open; Escape / Tab are handled while focus is inside it). */
  panelRef: RefObject<HTMLElement | null>
  /** Element that contains the trigger AND the panel; a pointerdown outside it closes. Defaults to the panel (context menus). */
  wrapRef?: RefObject<HTMLElement | null>
}

/**
 * Shared behaviour of the hand-rolled popovers and menus (non-modal, so no inert page behind them):
 *  - opening moves focus into the panel (first menu item / first focusable / the panel itself);
 *  - Escape closes and returns focus to whatever had it before (the trigger);
 *  - a pointerdown outside closes (focus then stays where the user clicked);
 *  - RULE: Tab and Shift+Tab are contained in the panel and cycle through its focusable elements, Escape is the way out;
 *  - in a `menu`, ArrowUp / ArrowDown / Home / End rove across the enabled menu items.
 * The global shortcuts stay quiet meanwhile because `useShortcuts` ignores keys while a role=dialog / role=menu is mounted.
 * Spread `triggerProps` on the trigger button and `panelProps` on the panel.
 */
export function usePopover({ open, onClose, menu = false, panelRef, wrapRef }: Options) {
  const id = useId()
  const close = useRef(onClose)
  useEffect(() => {
    close.current = onClose
  })

  useEffect(() => {
    if (!open) return
    const panel = panelRef.current
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null
    let restore = true

    const first = menu ? panel?.querySelector<HTMLElement>(MENU_ITEMS) : panel?.querySelector<HTMLElement>(FOCUSABLE)
    ;(first ?? panel)?.focus({ preventScroll: true })

    const onDown = (e: PointerEvent) => {
      const box = (wrapRef ?? panelRef).current
      if (box && !box.contains(e.target as Node)) {
        restore = false
        close.current()
      }
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        close.current()
        return
      }
      const p = panelRef.current
      if (!p || !p.contains(document.activeElement)) return
      if (e.key === 'Tab') {
        const els = [...p.querySelectorAll<HTMLElement>(FOCUSABLE)]
        const next = els[tabIndexIn(els.indexOf(document.activeElement as HTMLElement), els.length, e.shiftKey)]
        e.preventDefault()
        ;(next ?? p).focus()
      } else if (menu) {
        const items = [...p.querySelectorAll<HTMLElement>(MENU_ITEMS)]
        const to = rovingIndex(items.indexOf(document.activeElement as HTMLElement), items.length, e.key)
        if (to === null) return
        e.preventDefault()
        items[to].focus()
      }
    }
    document.addEventListener('pointerdown', onDown)
    window.addEventListener('keydown', onKey, true)
    return () => {
      document.removeEventListener('pointerdown', onDown)
      window.removeEventListener('keydown', onKey, true)
      // the focused element is gone with the panel: hand focus back (unless the user clicked elsewhere)
      const a = document.activeElement
      if (restore && (!a || a === document.body) && opener?.isConnected) opener.focus({ preventScroll: true })
    }
  }, [open, menu, panelRef, wrapRef])

  return {
    triggerProps: { 'aria-haspopup': (menu ? 'menu' : 'dialog') as 'menu' | 'dialog', 'aria-expanded': open, 'aria-controls': open ? id : undefined },
    panelProps: { id, tabIndex: -1 },
  }
}
