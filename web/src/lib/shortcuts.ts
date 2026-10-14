/**
 * User-customisable shortcuts (docs/09 §5.3): overrides are a map `action id -> chord` stored in the server
 * settings (`settings.shortcuts`, shared by the desktop app and the LAN WebUI). This module turns the defaults
 * (`BINDINGS`) plus the overrides into the effective bindings and plans edits with conflict detection. Pure
 * functions; the sync hook and the settings editor live next to the UI.
 *
 * Rules:
 * - an override replaces the primary key of its action (hidden aliases of that action are dropped);
 * - `fixed` bindings (Escape, help `?`, palette Ctrl+K, the hold-to-zoom key, ...) cannot be rebound and their
 *   chords cannot be taken by another action in the same scope;
 * - two different actions never share a chord in the same scope.
 */
import { useSyncExternalStore } from 'react'
import {
  BINDINGS,
  bindingsVersion,
  normalizeChord,
  setActiveBindings,
  subscribeBindings,
  getBindings,
  type Binding,
  type Group,
  type Scope,
} from './keymap'

export type Overrides = Record<string, string>

/** Chords nobody may take: Escape is "close / cancel", Shift+arrows extend the grid selection. */
const RESERVED = new Set(['escape', 'shift+arrowleft', 'shift+arrowright', 'shift+arrowup', 'shift+arrowdown'])

export function isReservedChord(chord: string): boolean {
  return RESERVED.has(chord)
}

/** Ids that may be rebound: not fixed, not hold-style. */
export function isCustomizable(id: string, defaults: Binding[] = BINDINGS): boolean {
  const own = defaults.filter((b) => b.id === id)
  return own.length > 0 && own.every((b) => !b.fixed && !b.hold)
}

/** Ids that can be rebound, in registry order. */
export function customizableIds(defaults: Binding[] = BINDINGS): string[] {
  return [...new Set(defaults.map((b) => b.id))].filter((id) => isCustomizable(id, defaults))
}

/** The default primary chord of an action (first visible binding). */
export function defaultChord(id: string, defaults: Binding[] = BINDINGS): string | undefined {
  return (defaults.find((b) => b.id === id && !b.hidden) ?? defaults.find((b) => b.id === id))?.keys[0]
}

/** Drops unknown ids, fixed actions, invalid / reserved chords and values equal to the default. */
export function sanitizeOverrides(raw: unknown, defaults: Binding[] = BINDINGS): Overrides {
  const out: Overrides = {}
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) return out
  for (const [id, v] of Object.entries(raw as Record<string, unknown>)) {
    if (typeof v !== 'string' || !isCustomizable(id, defaults)) continue
    const chord = normalizeChord(v)
    if (!chord || isReservedChord(chord) || chord === defaultChord(id, defaults)) continue
    out[id] = chord
  }
  return out
}

/** Defaults with the overrides applied. Conflicting overrides are not filtered here; see `planAssign`. */
export function applyOverrides(defaults: Binding[], overrides: Overrides): Binding[] {
  const done = new Set<string>()
  const out: Binding[] = []
  for (const b of defaults) {
    const chord = overrides[b.id]
    if (!chord || b.fixed || b.hold) {
      out.push(b)
      continue
    }
    if (b.hidden || done.has(b.id)) continue // aliases of an overridden action
    done.add(b.id)
    out.push({ ...b, keys: [chord] })
  }
  return out
}

/** Whether two bindings share a scope. */
function sharesScope(a: Binding, b: Binding): boolean {
  return a.scopes.some((s) => b.scopes.includes(s))
}

/** Bindings of other actions that use `chord` in a scope where action `id` is active. */
export function findConflicts(bindings: Binding[], id: string, chord: string): Binding[] {
  const own = bindings.filter((b) => b.id === id)
  const seen = new Set<string>()
  return bindings.filter((b) => {
    if (b.id === id || !b.keys.includes(chord) || !own.some((o) => sharesScope(o, b)) || seen.has(b.id)) return false
    seen.add(b.id)
    return true
  })
}

/** The chord an action currently uses (primary key). */
export function currentChord(bindings: Binding[], id: string): string | undefined {
  return (bindings.find((b) => b.id === id && !b.hidden) ?? bindings.find((b) => b.id === id))?.keys[0]
}

function withOverride(overrides: Overrides, id: string, chord: string, defaults: Binding[]): Overrides {
  const next = { ...overrides }
  if (chord === defaultChord(id, defaults)) delete next[id]
  else next[id] = chord
  return next
}

export type AssignPlan =
  | { kind: 'invalid' }
  | { kind: 'same' }
  | { kind: 'ok'; overrides: Overrides }
  | {
      kind: 'conflict'
      /** the action(s) already using the chord */
      with: Binding[]
      /** swapping is possible: the other action takes this action's current chord */
      swap?: Overrides
    }

/** Plans giving action `id` the chord `rawChord`. */
export function planAssign(
  overrides: Overrides,
  id: string,
  rawChord: string,
  defaults: Binding[] = BINDINGS,
): AssignPlan {
  const chord = normalizeChord(rawChord)
  if (!chord || isReservedChord(chord) || !isCustomizable(id, defaults)) return { kind: 'invalid' }
  const effective = applyOverrides(defaults, overrides)
  if (currentChord(effective, id) === chord) return { kind: 'same' }
  const conflicts = findConflicts(effective, id, chord)
  if (conflicts.length === 0) return { kind: 'ok', overrides: withOverride(overrides, id, chord, defaults) }
  const plan: AssignPlan = { kind: 'conflict', with: conflicts }
  const otherIds = [...new Set(conflicts.map((c) => c.id))]
  const old = currentChord(effective, id)
  if (otherIds.length === 1 && old && isCustomizable(otherIds[0], defaults)) {
    const other = otherIds[0]
    const next = withOverride(withOverride(overrides, id, chord, defaults), other, old, defaults)
    const eff = applyOverrides(defaults, next)
    if (findConflicts(eff, id, chord).length === 0 && findConflicts(eff, other, old).length === 0) plan.swap = next
  }
  return plan
}

/** Overrides after resetting one action to its default. */
export function resetOne(overrides: Overrides, id: string): Overrides {
  const next = { ...overrides }
  delete next[id]
  return next
}

/**
 * Merge-patch body (`PATCH /api/settings`) turning `prev` into `next`: changed chords are set, removed ones are
 * `null` (the server's merge patch deletes the key). Null when nothing changed.
 */
export function overridesPatch(prev: Overrides, next: Overrides): Record<string, string | null> | null {
  const out: Record<string, string | null> = {}
  for (const k of Object.keys(next)) if (prev[k] !== next[k]) out[k] = next[k]
  for (const k of Object.keys(prev)) if (!(k in next)) out[k] = null
  return Object.keys(out).length ? out : null
}

/** One editable row of the settings table. */
export interface ActionRow {
  id: string
  desc: string
  group: Group
  scopes: Scope[]
  chord: string
  defaultChord: string
  fixed: boolean
  overridden: boolean
}

/** One row per action id (hidden aliases left out), in registry order. */
export function actionRows(overrides: Overrides, defaults: Binding[] = BINDINGS): ActionRow[] {
  const effective = applyOverrides(defaults, overrides)
  const rows: ActionRow[] = []
  for (const b of effective) {
    if (b.hidden || rows.some((r) => r.id === b.id)) continue
    rows.push({
      id: b.id,
      desc: b.desc,
      group: b.group,
      scopes: b.scopes,
      chord: b.keys[0],
      defaultChord: defaultChord(b.id, defaults) ?? b.keys[0],
      fixed: !isCustomizable(b.id, defaults),
      overridden: !!overrides[b.id],
    })
  }
  return rows
}

/** Re-renders when the effective bindings change; returns them. */
export function useBindings(): Binding[] {
  useSyncExternalStore(subscribeBindings, bindingsVersion)
  return getBindings()
}

/** Make `overrides` (raw settings value) the active key map. */
export function activateOverrides(raw: unknown): void {
  const overrides = sanitizeOverrides(raw)
  setActiveBindings(Object.keys(overrides).length ? applyOverrides(BINDINGS, overrides) : BINDINGS)
}

/** Display name of an action, e.g. "Rate 3" for `rate.3` (digits are part of the id, not of the translation). */
export function actionLabel(id: string, desc: string, t: (key: string) => string): string {
  const digit = id.match(/^(?:rate|rateNext)\.(\d)$/)
  return digit ? `${t(`keys.${desc}`)} ${digit[1]}` : t(`keys.${desc}`)
}
