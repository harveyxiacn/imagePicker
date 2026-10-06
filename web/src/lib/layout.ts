/** Responsive layout helpers (doc 08 section 5). Pure functions: the hook lives in `useLayout.ts`. */

/** Phone layout applies strictly below this width (Tailwind `md`). */
export const MOBILE_BREAKPOINT = 768
export const TABLET_BREAKPOINT = 1024
/** Minimum touch target edge (px). */
export const TOUCH_TARGET = 44

export type LayoutClass = 'mobile' | 'tablet' | 'desktop'

export function layoutClass(width: number): LayoutClass {
  if (width < MOBILE_BREAKPOINT) return 'mobile'
  if (width < TABLET_BREAKPOINT) return 'tablet'
  return 'desktop'
}

export const isMobileWidth = (width: number): boolean => width < MOBILE_BREAKPOINT

export type NavTab = 'library' | 'cull' | 'people' | 'settings'
export const NAV_TABS: NavTab[] = ['library', 'cull', 'people', 'settings']

/** Routes that show the bottom navigation (full-screen flows such as edit and login hide it). */
export function showsBottomNav(pathname: string): boolean {
  if (pathname === '/login') return false
  if (/^\/s\/\d+\/edit\//.test(pathname)) return false
  if (/^\/s\/\d+\/besttake\//.test(pathname)) return false
  return true
}

/** Which tab is highlighted for a route (`view` = the library view of the current session). */
export function activeNavTab(pathname: string, view: string): NavTab {
  if (pathname.startsWith('/settings')) return 'settings'
  if (/^\/s\/\d+\/people/.test(pathname)) return 'people'
  if (/^\/s\/\d+\/?$/.test(pathname)) return view === 'loupe' || view === 'group' || view === 'compare' ? 'cull' : 'library'
  return 'library'
}

/** Route for a tab; tabs that need a session are `null` until one exists. */
export function navTarget(tab: NavTab, sessionId: number | null): string | null {
  switch (tab) {
    case 'library':
      return sessionId === null ? '/' : `/s/${sessionId}`
    case 'cull':
      return sessionId === null ? null : `/s/${sessionId}`
    case 'people':
      return sessionId === null ? null : `/s/${sessionId}/people`
    case 'settings':
      return '/settings'
  }
}

/** Session id carried by a library route, if any. */
export function sessionFromPath(pathname: string): number | null {
  const m = /^\/s\/(\d+)/.exec(pathname)
  return m ? Number(m[1]) : null
}

/** CSS values that keep content clear of the notch / gesture bar. */
export const SAFE_AREA = {
  top: 'env(safe-area-inset-top, 0px)',
  bottom: 'env(safe-area-inset-bottom, 0px)',
  left: 'env(safe-area-inset-left, 0px)',
  right: 'env(safe-area-inset-right, 0px)',
} as const

/** Clamp a bottom-sheet height (px) between a minimum and a viewport fraction. */
export function sheetHeight(viewportH: number, wanted: number, maxFraction = 0.9, min = 160): number {
  return Math.round(Math.max(min, Math.min(wanted, viewportH * maxFraction)))
}

/** Settle a dragged sheet: close when pulled down far/fast enough, expand when flicked up. */
export function settleSheet(dy: number, velocityY: number, height: number): 'close' | 'stay' | 'expand' {
  if (dy > height * 0.3 || velocityY > 0.6) return 'close'
  if (dy < -60 || velocityY < -0.6) return 'expand'
  return 'stay'
}
