/**
 * Escape handling for the floating sidebar / inspector (< lg). Pure so it can be tested: the overlay should
 * take Escape only when nothing more specific (a dialog, a menu, a text field) is using it.
 */
export function overlayOwnsEscape(opts: { panelOpen: boolean; typing: boolean; dialogOpen: boolean; menuOpen: boolean }): boolean {
  return opts.panelOpen && !opts.typing && !opts.dialogOpen && !opts.menuOpen
}
