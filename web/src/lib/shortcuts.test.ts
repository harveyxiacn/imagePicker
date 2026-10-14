import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  BINDINGS,
  dispatchKey,
  findBinding,
  formatSpec,
  getBindings,
  helpGroups,
  type Binding,
  type Handlers,
} from './keymap'
import { actionItems } from './palette'
import {
  actionLabel,
  actionRows,
  activateOverrides,
  applyOverrides,
  currentChord,
  customizableIds,
  defaultChord,
  findConflicts,
  isCustomizable,
  overridesPatch,
  planAssign,
  resetOne,
  sanitizeOverrides,
} from './shortcuts'

const ev = (key: string, extra: Partial<KeyboardEvent> = {}) => ({ key, ...extra })
const t = (k: string) => k.split('.').pop() ?? k

afterEach(() => activateOverrides({}))

describe('fixed bindings', () => {
  it('Escape, help, palette and the hold-to-zoom key cannot be rebound', () => {
    for (const id of ['nav.back', 'edit.close', 'help.toggle', 'palette.toggle', 'zoom.hold', 'zoom.open'])
      expect(isCustomizable(id), id).toBe(false)
    for (const id of ['flag.pick', 'edit.undo', 'edit.redo', 'rate.3', 'assistant.toggle']) expect(isCustomizable(id), id).toBe(true)
    expect(isCustomizable('does.not.exist')).toBe(false)
  })

  it('keeps the server list of customizable actions (settings.rs) in sync with the key map', () => {
    const src = readFileSync(fileURLToPath(new URL('../../../crates/ip-core/src/settings.rs', import.meta.url)), 'utf8')
    const block = /SHORTCUT_ACTIONS: &\[&str\] = &\[([\s\S]*?)\];/.exec(src)?.[1] ?? ''
    const server = [...block.matchAll(/"([^"]+)"/g)].map((m) => m[1])
    expect([...server].sort()).toEqual([...customizableIds()].sort())
  })
})

describe('sanitizeOverrides', () => {
  it('keeps valid overrides, normalised', () => {
    expect(sanitizeOverrides({ 'flag.pick': 'Shift+Mod+P' })).toEqual({ 'flag.pick': 'mod+shift+p' })
  })
  it('drops unknown / fixed ids, invalid or reserved chords, non-strings and default values', () => {
    expect(
      sanitizeOverrides({
        'nope.id': 'a',
        'help.toggle': 'h',
        'nav.back': 'b',
        'flag.pick': 'escape',
        'flag.reject': 'mod+',
        'flag.none': 5,
        'view.grid': 'g', // the default
        'view.loupe': 'shift+arrowleft',
      }),
    ).toEqual({})
    expect(sanitizeOverrides(null)).toEqual({})
    expect(sanitizeOverrides([['flag.pick', 'k']])).toEqual({})
  })
})

describe('resolver with overrides', () => {
  it('the new chord triggers the action and the old one no longer does', () => {
    activateOverrides({ 'flag.pick': 'k' })
    const pick = vi.fn()
    const handlers: Handlers = { 'flag.pick': pick }
    expect(dispatchKey(ev('p'), 'grid', handlers)).toBe(false)
    expect(dispatchKey(ev('k'), 'grid', handlers)).toBe(true)
    expect(pick).toHaveBeenCalledOnce()
    expect(findBinding('k', 'loupe')?.id).toBe('flag.pick')
  })

  it('an override drops the hidden aliases of the action', () => {
    activateOverrides({ 'edit.redo': 'mod+r' })
    expect(findBinding('mod+r', 'grid')?.id).toBe('edit.redo')
    expect(findBinding('mod+shift+z', 'grid')).toBeUndefined()
    expect(findBinding('mod+y', 'grid')).toBeUndefined()
  })

  it('fixed bindings ignore overrides', () => {
    const eff = applyOverrides(BINDINGS, { 'help.toggle': 'h', 'zoom.hold': 'q' })
    expect(eff).toEqual(BINDINGS)
  })

  it('help overlay and palette hints follow the overrides', () => {
    activateOverrides({ 'flag.pick': 'k' })
    const help = helpGroups().flatMap((g) => g.items)
    expect(help.find((i) => i.desc === 'flagPick')?.keys).toEqual([['K']])
    const item = actionItems({ scope: 'grid', t, available: () => true }).find((i) => i.id === 'flag.pick')
    expect(item?.keys).toEqual([['K']])
  })

  it('platform display stays: mod renders as Ctrl here (⌘ on macOS)', () => {
    activateOverrides({ 'flag.pick': 'mod+shift+p' })
    const chord = currentChord(getBindings(), 'flag.pick')!
    expect(chord).toBe('mod+shift+p') // stored platform-neutral
    expect(formatSpec(chord)).toEqual([expect.stringMatching(/^(Ctrl|⌘)$/), expect.stringMatching(/^(Shift|⇧)$/), 'P'])
    // Ctrl and Meta both dispatch to `mod`
    expect(dispatchKey(ev('p', { ctrlKey: true, shiftKey: true }), 'grid', { 'flag.pick': () => undefined })).toBe(true)
    expect(dispatchKey(ev('p', { metaKey: true, shiftKey: true }), 'grid', { 'flag.pick': () => undefined })).toBe(true)
  })

  it('reset: empty overrides give the defaults back (same array)', () => {
    activateOverrides({ 'flag.pick': 'k' })
    expect(getBindings()).not.toBe(BINDINGS)
    activateOverrides({})
    expect(getBindings()).toBe(BINDINGS)
    expect(findBinding('p', 'grid')?.id).toBe('flag.pick')
  })

  it('actionRows lists one row per action with default, custom and fixed flags', () => {
    const rows = actionRows({ 'flag.pick': 'k' })
    expect(rows.filter((r) => r.id === 'edit.redo')).toHaveLength(1)
    expect(rows.find((r) => r.id === 'flag.pick')).toMatchObject({ chord: 'k', defaultChord: 'p', overridden: true, fixed: false })
    expect(rows.find((r) => r.id === 'help.toggle')).toMatchObject({ fixed: true, overridden: false })
    expect(rows.find((r) => r.id === 'zoom.open')).toBeUndefined() // hidden alias
    expect(actionLabel('rate.3', 'rate', t)).toBe('rate 3')
  })
})

describe('conflict detection', () => {
  it('finds another action with the same chord in an overlapping scope', () => {
    const plan = planAssign({}, 'flag.pick', 'x')
    expect(plan.kind).toBe('conflict')
    if (plan.kind === 'conflict') expect(plan.with.map((b) => b.id)).toEqual(['flag.reject'])
  })

  it('the same chord in a different scope is not a conflict', () => {
    // `s` is compare.sync (compare, group); stacks.toggle only lives in the grid
    expect(findConflicts(BINDINGS, 'stacks.toggle', 's')).toEqual([])
    const plan = planAssign({}, 'compare.sync', 'enter') // group.pick lives in the group scope, which compare.sync shares
    expect(plan.kind).toBe('conflict')
    if (plan.kind === 'conflict') expect(plan.with.map((x) => x.id)).toEqual(['group.pick'])
  })

  it('offers a swap: the other action takes this action\'s chord', () => {
    const plan = planAssign({}, 'flag.pick', 'x')
    if (plan.kind !== 'conflict' || !plan.swap) throw new Error('expected a swappable conflict')
    expect(plan.swap).toEqual({ 'flag.pick': 'x', 'flag.reject': 'p' })
    const eff = applyOverrides(BINDINGS, plan.swap)
    expect(findBinding('p', 'grid', eff)?.id).toBe('flag.reject')
    expect(findBinding('x', 'grid', eff)?.id).toBe('flag.pick')
  })

  it('a conflict with a fixed binding cannot be swapped', () => {
    for (const chord of ['mod+k', '?', 'z']) {
      const plan = planAssign({}, 'flag.pick', chord)
      expect(plan.kind, chord).toBe('conflict')
      if (plan.kind === 'conflict') expect(plan.swap, chord).toBeUndefined()
    }
  })

  it('several holders of the chord cannot be swapped either', () => {
    const plan = planAssign({}, 'nav.prev', 'tab') // compare.swap and view.sidebar
    expect(plan.kind).toBe('conflict')
    if (plan.kind === 'conflict') {
      expect(new Set(plan.with.map((b) => b.id))).toEqual(new Set(['compare.swap', 'view.sidebar']))
      expect(plan.swap).toBeUndefined()
    }
  })

  it('a swap that would create a new conflict is not offered', () => {
    // view.sidebar (Tab: grid, loupe) -> ArrowRight (nav.next, all scopes). Swapping would give nav.next the Tab key
    // in compare / group, where compare.swap already holds it.
    const plan = planAssign({}, 'view.sidebar', 'arrowright')
    expect(plan.kind).toBe('conflict')
    if (plan.kind === 'conflict') {
      expect(plan.with.map((x) => x.id)).toEqual(['nav.next'])
      expect(plan.swap).toBeUndefined()
    }
  })

  it('rejects invalid input, fixed actions and reserved chords', () => {
    expect(planAssign({}, 'flag.pick', 'mod+').kind).toBe('invalid')
    expect(planAssign({}, 'flag.pick', 'escape').kind).toBe('invalid')
    expect(planAssign({}, 'flag.pick', 'shift+arrowleft').kind).toBe('invalid')
    expect(planAssign({}, 'help.toggle', 'h').kind).toBe('invalid')
    expect(planAssign({}, 'nav.back', 'q').kind).toBe('invalid')
  })

  it('assigning the current chord is a no-op, a free chord is stored', () => {
    expect(planAssign({}, 'flag.pick', 'p').kind).toBe('same')
    expect(planAssign({}, 'flag.pick', 'k')).toEqual({ kind: 'ok', overrides: { 'flag.pick': 'k' } })
    expect(planAssign({ 'flag.pick': 'k' }, 'flag.pick', 'k').kind).toBe('same')
  })

  it('assigning the default chord again removes the override', () => {
    expect(planAssign({ 'flag.pick': 'k' }, 'flag.pick', 'p')).toEqual({ kind: 'ok', overrides: {} })
  })

  it('the freed default chord can be taken by another action', () => {
    const plan = planAssign({ 'flag.pick': 'k' }, 'flag.reject', 'p')
    expect(plan).toEqual({ kind: 'ok', overrides: { 'flag.pick': 'k', 'flag.reject': 'p' } })
  })

  it('the registry has no conflicts with itself', () => {
    for (const id of customizableIds()) {
      const own = defaultChord(id)!
      expect(findConflicts(BINDINGS, id, own), id).toEqual([])
    }
  })

  it('a planned assignment never produces a duplicate chord in a scope', () => {
    const dup = (bs: Binding[]) => {
      const seen = new Map<string, string>()
      for (const b of bs)
        for (const s of b.scopes)
          for (const k of b.keys) {
            const key = `${s}:${k}`
            if (seen.has(key) && seen.get(key) !== b.id) return key
            seen.set(key, b.id)
          }
      return null
    }
    for (const [id, chord] of [['flag.pick', 'x'], ['view.grid', 'e'], ['nav.up', 'tab'], ['edit.undo', 'mod+y']] as const) {
      const plan = planAssign({}, id, chord)
      const next = plan.kind === 'ok' ? plan.overrides : plan.kind === 'conflict' ? plan.swap : undefined
      if (next) expect(dup(applyOverrides(BINDINGS, next)), `${id} -> ${chord}`).toBeNull()
    }
  })
})

describe('reset and persistence patch', () => {
  it('resetOne removes just that action', () => {
    expect(resetOne({ 'flag.pick': 'k', 'flag.none': 'n' }, 'flag.pick')).toEqual({ 'flag.none': 'n' })
  })
  it('overridesPatch sets changed chords and nulls removed ones (merge patch)', () => {
    expect(overridesPatch({}, { 'flag.pick': 'k' })).toEqual({ 'flag.pick': 'k' })
    expect(overridesPatch({ 'flag.pick': 'k', 'flag.none': 'n' }, {})).toEqual({ 'flag.pick': null, 'flag.none': null })
    expect(overridesPatch({ 'flag.pick': 'k' }, { 'flag.pick': 'j' })).toEqual({ 'flag.pick': 'j' })
    expect(overridesPatch({ 'flag.pick': 'k' }, { 'flag.pick': 'k' })).toBeNull()
  })
  it('swap patch touches exactly the two actions', () => {
    const plan = planAssign({}, 'flag.pick', 'x')
    if (plan.kind !== 'conflict' || !plan.swap) throw new Error('expected swap')
    expect(overridesPatch({}, plan.swap)).toEqual({ 'flag.pick': 'x', 'flag.reject': 'p' })
  })
})
