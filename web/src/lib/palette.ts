/**
 * Command palette (Ctrl+K) model: fuzzy search + ranking over the actions of the central keymap
 * (`BINDINGS`, the same registry the help overlay and the settings table are generated from)
 * and a few navigation targets. Pure functions; the UI lives in `components/CommandPalette.tsx`.
 */
import { BINDINGS, formatSpec, type Binding, type Scope } from './keymap'

export interface PaletteItem {
  id: string
  kind: 'action' | 'nav'
  label: string
  /** group heading shown next to the label */
  group: string
  /** shortcut caps, e.g. [['Ctrl', 'Z']] */
  keys: string[][]
  /** extra searchable text (action id words) */
  extra: string
  /** route for navigation targets */
  path?: string
}

/** Score of one query token against `text` (higher is better), or null when it does not match. */
export function fuzzyScore(token: string, text: string): number | null {
  const q = token.toLowerCase()
  const t = text.toLowerCase()
  if (!q) return 0
  const idx = t.indexOf(q)
  if (idx >= 0) {
    let s = 1000 - idx * 5 - (t.length - q.length)
    if (idx === 0) s += 200
    else if (!/[\p{L}\p{N}]/u.test(t[idx - 1])) s += 100
    return s
  }
  // subsequence: every query character appears in order
  let ti = 0
  let score = 0
  let prev = -2
  for (const ch of q) {
    const at = t.indexOf(ch, ti)
    if (at < 0) return null
    score += 10
    if (at === prev + 1) score += 15
    else if (prev >= 0) score -= Math.min(8, at - prev - 1)
    if (at === 0 || !/[\p{L}\p{N}]/u.test(t[at - 1])) score += 10
    prev = at
    ti = at + 1
  }
  return Math.max(1, Math.min(score, 400))
}

/** Every whitespace separated token has to match the label (weighted) or the extra text. */
export function scoreItem(item: Pick<PaletteItem, 'label' | 'extra' | 'group'>, query: string): number | null {
  const tokens = query.trim().split(/\s+/).filter(Boolean)
  if (tokens.length === 0) return 0
  let total = 0
  for (const tok of tokens) {
    const a = fuzzyScore(tok, item.label)
    const b = fuzzyScore(tok, item.extra)
    const c = fuzzyScore(tok, item.group)
    const best = Math.max(a === null ? -Infinity : a + 100, b === null ? -Infinity : b, c === null ? -Infinity : c - 200)
    if (best === -Infinity) return null
    total += best
  }
  return total
}

/** Matching items best-first (ties keep the original order); an empty query keeps the original order. */
export function rankItems<T extends Pick<PaletteItem, 'label' | 'extra' | 'group'>>(items: T[], query: string, limit = 50): T[] {
  const scored: { item: T; score: number; i: number }[] = []
  items.forEach((item, i) => {
    const score = scoreItem(item, query)
    if (score !== null) scored.push({ item, score, i })
  })
  scored.sort((x, y) => y.score - x.score || x.i - y.i)
  return scored.slice(0, limit).map((s) => s.item)
}

export interface ActionItemOptions {
  scope: Scope
  /** translate `keys.<desc>`, `keyGroup.<group>` etc. */
  t: (key: string) => string
  /** only actions the current screen can run */
  available: (id: string) => boolean
  bindings?: Binding[]
}

const SKIP = new Set(['palette.toggle'])

/** One palette entry per action id of the keymap that is active in `scope` and runnable right now. */
export function actionItems({ scope, t, available, bindings = BINDINGS }: ActionItemOptions): PaletteItem[] {
  const byId = new Map<string, PaletteItem>()
  for (const b of bindings) {
    if (b.hold || SKIP.has(b.id) || !b.scopes.includes(scope) || !available(b.id)) continue
    const digit = b.id.match(/^(rate|rateNext)\.(\d)$/)
    const label = digit ? `${t(`keys.${b.desc}`)} ${digit[2]}` : t(`keys.${b.desc}`)
    const keys = b.hidden ? [] : b.keys.map(formatSpec)
    const existing = byId.get(b.id)
    if (existing) {
      existing.keys.push(...keys)
      continue
    }
    byId.set(b.id, {
      id: b.id,
      kind: 'action',
      label,
      group: t(`keyGroup.${b.group}`),
      keys,
      extra: b.id.replace(/[._]/g, ' '),
    })
  }
  return [...byId.values()]
}

export interface NavTarget {
  id: string
  /** i18n key under `palette.`  */
  label: string
  path: string
}

/** Pages reachable from the palette; session pages only when a library is open. */
export function navTargets(sessionId: number | null): NavTarget[] {
  const out: NavTarget[] = [
    { id: 'goto.home', label: 'goHome', path: '/' },
    { id: 'goto.settings', label: 'goSettings', path: '/settings' },
    { id: 'goto.taste', label: 'goTaste', path: '/taste' },
  ]
  if (sessionId !== null) {
    out.push(
      { id: 'goto.library', label: 'goLibrary', path: `/s/${sessionId}` },
      { id: 'goto.people', label: 'goPeople', path: `/s/${sessionId}/people` },
    )
  }
  return out
}

export function navItems(sessionId: number | null, t: (key: string) => string): PaletteItem[] {
  return navTargets(sessionId).map((n) => ({
    id: n.id,
    kind: 'nav' as const,
    label: t(`palette.${n.label}`),
    group: t('palette.navigation'),
    keys: [],
    extra: n.id.replace(/[._]/g, ' '),
    path: n.path,
  }))
}

/** All entries for the current screen: actions first, then navigation. */
export function buildItems(opts: ActionItemOptions & { sessionId: number | null }): PaletteItem[] {
  return [...actionItems(opts), ...navItems(opts.sessionId, opts.t)]
}

/** The keymap spec(s) that open the palette (so the global listener needs no hard-coded key). */
export function paletteSpecs(bindings: Binding[] = BINDINGS): string[] {
  return bindings.filter((b) => b.id === 'palette.toggle').flatMap((b) => b.keys)
}
