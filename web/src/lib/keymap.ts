/**
 * Central keymap registry. The `?` help overlay is generated from `BINDINGS`,
 * and `dispatchKey` resolves keyboard events to action ids.
 */

export type Scope = 'grid' | 'loupe' | 'compare' | 'group' | 'edit'
export type Group = 'rate' | 'flag' | 'nav' | 'view' | 'zoom' | 'edit' | 'ai' | 'misc'

export interface Binding {
  /** Action id handled by the UI (e.g. `rate.3`). */
  id: string
  /** Normalized key specs, e.g. `mod+shift+z`, `shift+3`, `arrowleft`. */
  keys: string[]
  /** Scopes in which the binding is active. */
  scopes: Scope[]
  group: Group
  /** i18n key (under `keys.`) describing the action. */
  desc: string
  /** Hold-style binding (handled outside of dispatch, listed in help only). */
  hold?: boolean
  /** Hidden from the help overlay (aliases). */
  hidden?: boolean
}

const ALL: Scope[] = ['grid', 'loupe', 'compare', 'group']
const NAV: Scope[] = ['grid', 'loupe', 'compare', 'group']
const WITH_EDIT: Scope[] = [...ALL, 'edit']

export const COLOR_KEYS = { '6': 'red', '7': 'yellow', '8': 'green', '9': 'blue' } as const

function rating(): Binding[] {
  const out: Binding[] = []
  for (let n = 0; n <= 5; n++) {
    out.push({ id: `rate.${n}`, keys: [String(n)], scopes: ALL, group: 'rate', desc: 'rate' })
    out.push({ id: `rateNext.${n}`, keys: [`shift+${n}`], scopes: ALL, group: 'rate', desc: 'rateNext' })
  }
  return out
}

export const BINDINGS: Binding[] = [
  ...rating(),
  { id: 'flag.pick', keys: ['p'], scopes: ALL, group: 'flag', desc: 'flagPick' },
  { id: 'flag.reject', keys: ['x'], scopes: ALL, group: 'flag', desc: 'flagReject' },
  { id: 'flag.none', keys: ['u'], scopes: ALL, group: 'flag', desc: 'flagNone' },
  { id: 'color.red', keys: ['6'], scopes: ALL, group: 'flag', desc: 'colorRed' },
  { id: 'color.yellow', keys: ['7'], scopes: ALL, group: 'flag', desc: 'colorYellow' },
  { id: 'color.green', keys: ['8'], scopes: ALL, group: 'flag', desc: 'colorGreen' },
  { id: 'color.blue', keys: ['9'], scopes: ALL, group: 'flag', desc: 'colorBlue' },

  { id: 'nav.prev', keys: ['arrowleft'], scopes: NAV, group: 'nav', desc: 'prev' },
  { id: 'nav.next', keys: ['arrowright'], scopes: NAV, group: 'nav', desc: 'next' },
  { id: 'nav.up', keys: ['arrowup'], scopes: ['grid'], group: 'nav', desc: 'up' },
  { id: 'nav.down', keys: ['arrowdown'], scopes: ['grid'], group: 'nav', desc: 'down' },
  { id: 'nav.first', keys: ['home'], scopes: NAV, group: 'nav', desc: 'first' },
  { id: 'nav.last', keys: ['end'], scopes: NAV, group: 'nav', desc: 'last' },
  { id: 'nav.open', keys: ['enter'], scopes: ['grid'], group: 'nav', desc: 'open' },
  { id: 'nav.back', keys: ['escape'], scopes: ALL, group: 'nav', desc: 'back' },

  { id: 'view.grid', keys: ['g'], scopes: ALL, group: 'view', desc: 'viewGrid' },
  { id: 'view.loupe', keys: ['e'], scopes: ALL, group: 'view', desc: 'viewLoupe' },
  { id: 'view.compare', keys: ['c'], scopes: ALL, group: 'view', desc: 'viewCompare' },
  { id: 'view.group', keys: ['b'], scopes: ALL, group: 'view', desc: 'viewGroup' },
  { id: 'view.fullscreen', keys: ['f'], scopes: ALL, group: 'view', desc: 'fullscreen' },
  { id: 'view.sidebar', keys: ['tab'], scopes: ['grid', 'loupe'], group: 'view', desc: 'sidebar' },
  { id: 'compare.swap', keys: ['tab'], scopes: ['compare', 'group'], group: 'view', desc: 'swap' },
  { id: 'compare.sync', keys: ['s'], scopes: ['compare', 'group'], group: 'view', desc: 'sync' },
  { id: 'zoom.toggle', keys: ['space'], scopes: ['loupe', 'compare', 'group'], group: 'zoom', desc: 'zoomToggle' },
  { id: 'zoom.open', keys: ['space'], scopes: ['grid'], group: 'zoom', desc: 'open', hidden: true },
  { id: 'zoom.hold', keys: ['z'], scopes: ['loupe', 'compare', 'group'], group: 'zoom', desc: 'zoomHold', hold: true },

  { id: 'edit.undo', keys: ['mod+z'], scopes: WITH_EDIT, group: 'edit', desc: 'undo' },
  { id: 'edit.redo', keys: ['mod+shift+z'], scopes: WITH_EDIT, group: 'edit', desc: 'redo' },
  { id: 'edit.redo', keys: ['mod+y'], scopes: WITH_EDIT, group: 'edit', desc: 'redo', hidden: true },
  { id: 'edit.open', keys: ['d'], scopes: ['grid', 'loupe'], group: 'edit', desc: 'editOpen' },
  { id: 'edit.close', keys: ['escape'], scopes: ['edit'], group: 'edit', desc: 'editClose' },
  { id: 'edit.close', keys: ['d'], scopes: ['edit'], group: 'edit', desc: 'editClose', hidden: true },
  { id: 'edit.before', keys: ['\\'], scopes: ['edit'], group: 'edit', desc: 'beforeAfter' },
  { id: 'edit.mask', keys: ['o'], scopes: ['edit'], group: 'edit', desc: 'maskOverlay' },
  { id: 'edit.crop', keys: ['r'], scopes: ['edit'], group: 'edit', desc: 'cropMode' },
  { id: 'edit.brush', keys: ['shift+e'], scopes: ['edit'], group: 'edit', desc: 'brushErase' },
  { id: 'edit.copy', keys: ['mod+shift+c'], scopes: ['edit'], group: 'edit', desc: 'copySettings' },
  { id: 'edit.paste', keys: ['mod+shift+v'], scopes: ['edit'], group: 'edit', desc: 'pasteSettings' },
  { id: 'select.all', keys: ['mod+a'], scopes: ['grid'], group: 'edit', desc: 'selectAll' },
  { id: 'ai.accept', keys: ['a'], scopes: ALL, group: 'ai', desc: 'aiAccept' },
  { id: 'ai.acceptAll', keys: ['mod+shift+a'], scopes: ALL, group: 'ai', desc: 'aiAcceptAll' },
  { id: 'stacks.toggle', keys: ['s'], scopes: ['grid'], group: 'ai', desc: 'stackToggle' },
  { id: 'stacks.toggleAll', keys: ['shift+s'], scopes: ['grid'], group: 'ai', desc: 'stackToggleAll' },
  { id: 'group.prev', keys: [','], scopes: ['grid', 'group'], group: 'ai', desc: 'groupPrev' },
  { id: 'group.next', keys: ['.'], scopes: ['grid', 'group'], group: 'ai', desc: 'groupNext' },
  { id: 'group.pick', keys: ['enter'], scopes: ['group'], group: 'ai', desc: 'groupPick' },
  { id: 'group.pickNext', keys: ['shift+enter'], scopes: ['group'], group: 'ai', desc: 'groupPickNext' },
  { id: 'faces.toggle', keys: ['shift+f'], scopes: ['loupe', 'compare', 'group'], group: 'ai', desc: 'facesToggle' },
  { id: 'personFilter.open', keys: ['shift+p'], scopes: ALL, group: 'ai', desc: 'personFilter' },
  { id: 'export.open', keys: ['mod+e'], scopes: WITH_EDIT, group: 'misc', desc: 'export' },
  { id: 'help.toggle', keys: ['?'], scopes: WITH_EDIT, group: 'misc', desc: 'help' },
]

export interface KeyEventLike {
  key: string
  code?: string
  ctrlKey?: boolean
  metaKey?: boolean
  altKey?: boolean
  shiftKey?: boolean
}

/** Normalizes an event to a key spec string such as `mod+shift+z` or `shift+3`. */
export function eventToSpec(e: KeyEventLike): string {
  let base: string
  const digit = e.code?.match(/^(?:Digit|Numpad)(\d)$/)
  if (digit) base = digit[1]
  else if (e.code === 'Space' || e.key === ' ') base = 'space'
  else base = e.key.toLowerCase()
  const mods: string[] = []
  if (e.ctrlKey || e.metaKey) mods.push('mod')
  if (e.altKey) mods.push('alt')
  // Shift is implied for printable symbols such as `?` (layout dependent).
  const symbol = base.length === 1 && !/[a-z0-9]/.test(base)
  if (e.shiftKey && !symbol) mods.push('shift')
  return [...mods, base].join('+')
}

export function findBinding(spec: string, scope: Scope, bindings: Binding[] = BINDINGS): Binding | undefined {
  return bindings.find((b) => !b.hold && b.scopes.includes(scope) && b.keys.includes(spec))
}

/** Whether the event originates from a text-entry element (shortcuts are suspended). */
export function isTypingTarget(t: EventTarget | null): boolean {
  const el = t as HTMLElement | null
  if (!el || typeof el.tagName !== 'string') return false
  const tag = el.tagName
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || el.isContentEditable === true
}

/** Like `isTypingTarget`, but sliders / checkboxes keep global shortcuts working after being clicked. */
export function isTextEntryTarget(t: EventTarget | null): boolean {
  const el = t as HTMLInputElement | null
  if (!el || typeof el.tagName !== 'string') return false
  if (el.tagName === 'INPUT') return !['range', 'checkbox', 'radio', 'button', 'color'].includes(el.type)
  return isTypingTarget(t)
}

export type Handlers = Record<string, ((spec: string) => void) | undefined>

/** Resolve and run the action bound to `e` in `scope`. Returns true when handled. */
export function dispatchKey(
  e: KeyEventLike,
  scope: Scope,
  handlers: Handlers,
  bindings: Binding[] = BINDINGS,
): boolean {
  const spec = eventToSpec(e)
  const b = findBinding(spec, scope, bindings)
  if (!b) return false
  const h = handlers[b.id]
  if (!h) return false
  h(spec)
  return true
}

const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform)

const KEY_LABELS: Record<string, string> = {
  arrowleft: '←',
  arrowright: '→',
  arrowup: '↑',
  arrowdown: '↓',
  space: 'Space',
  escape: 'Esc',
  enter: 'Enter',
  tab: 'Tab',
  home: 'Home',
  end: 'End',
}

/** Human-readable key label array for rendering `<kbd>` caps. */
export function formatSpec(spec: string): string[] {
  return spec.split('+').map((p) => {
    if (p === 'mod') return isMac ? '⌘' : 'Ctrl'
    if (p === 'shift') return isMac ? '⇧' : 'Shift'
    if (p === 'alt') return isMac ? '⌥' : 'Alt'
    return KEY_LABELS[p] ?? p.toUpperCase()
  })
}

/** Registry grouped for the help overlay (hidden aliases removed, same actions merged). */
export function helpGroups(): { group: Group; items: { desc: string; keys: string[][]; scopes: Scope[] }[] }[] {
  const order: Group[] = ['rate', 'flag', 'nav', 'view', 'zoom', 'edit', 'ai', 'misc']
  return order
    .map((group) => {
      const items: { desc: string; keys: string[][]; scopes: Scope[] }[] = []
      for (const b of BINDINGS.filter((x) => x.group === group && !x.hidden)) {
        const keys = b.keys.map(formatSpec)
        const existing = items.find((i) => i.desc === b.desc && b.desc !== 'rate' && b.desc !== 'rateNext')
        if (existing) existing.keys.push(...keys)
        else items.push({ desc: b.desc, keys, scopes: b.scopes })
      }
      return { group, items: collapseRatings(items) }
    })
    .filter((g) => g.items.length > 0)
}

// Collapse rate.0..5 / rateNext.0..5 into single help rows "0-5" and "Shift+0-5".
function collapseRatings(items: { desc: string; keys: string[][]; scopes: Scope[] }[]) {
  const out: typeof items = []
  for (const it of items) {
    if (it.desc === 'rate' || it.desc === 'rateNext') {
      if (out.some((o) => o.desc === it.desc)) continue
      const shift = it.desc === 'rateNext'
      out.push({
        desc: it.desc,
        keys: [shift ? [...formatSpec('shift'), '0–5'] : ['0–5']],
        scopes: it.scopes,
      })
    } else out.push(it)
  }
  return out
}
