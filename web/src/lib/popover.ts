/** Pure helpers behind `usePopover` (keyboard roving / focus cycling) and `useKeyboardInset` (on-screen keyboard). */

/**
 * Next index for a `role="menu"` list: ArrowDown / ArrowUp wrap around, Home / End jump to the ends.
 * `current` is -1 when focus is not on an item yet. Returns null for keys that do not move the focus.
 */
export function rovingIndex(current: number, count: number, key: string): number | null {
  if (count <= 0) return null
  switch (key) {
    case 'ArrowDown':
      return current < 0 ? 0 : (current + 1) % count
    case 'ArrowUp':
      return current < 0 ? count - 1 : (current - 1 + count) % count
    case 'Home':
      return 0
    case 'End':
      return count - 1
    default:
      return null
  }
}

/** Where Tab / Shift+Tab goes when focus is trapped inside a popover with `count` focusable elements (wraps around). */
export function tabIndexIn(current: number, count: number, shift: boolean): number {
  if (count <= 0) return -1
  if (current < 0) return shift ? count - 1 : 0
  return shift ? (current - 1 + count) % count : (current + 1) % count
}

/**
 * Height the on-screen keyboard (or any browser UI) takes from the bottom of the layout viewport, from `visualViewport`.
 * Zero when the layout viewport already shrinks with the keyboard (`interactive-widget=resizes-content`).
 */
export function keyboardInset(layoutHeight: number, vv: { height: number; offsetTop: number } | null | undefined): number {
  if (!vv) return 0
  const inset = Math.round(layoutHeight - vv.height - vv.offsetTop)
  // small differences are URL-bar animation noise, not a keyboard
  return inset > 40 ? inset : 0
}
