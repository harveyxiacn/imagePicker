import { describe, expect, it, vi } from 'vitest'
import { BINDINGS, dispatchKey, eventToSpec, findBinding, helpGroups, normalizeChord, specsFor, type Handlers } from './keymap'

const ev = (key: string, extra: Partial<KeyboardEvent> = {}) => ({ key, ...extra })

describe('eventToSpec', () => {
  it('normalizes plain, modifier and digit keys', () => {
    expect(eventToSpec(ev('P'))).toBe('p')
    expect(eventToSpec(ev('z', { ctrlKey: true }))).toBe('mod+z')
    expect(eventToSpec(ev('Z', { metaKey: true, shiftKey: true }))).toBe('mod+shift+z')
    expect(eventToSpec(ev('ArrowLeft'))).toBe('arrowleft')
    expect(eventToSpec(ev(' ', { code: 'Space' }))).toBe('space')
  })

  it('uses the physical digit for shift+number (layout independent)', () => {
    expect(eventToSpec(ev('#', { code: 'Digit3', shiftKey: true }))).toBe('shift+3')
    expect(eventToSpec(ev('3', { code: 'Digit3' }))).toBe('3')
  })

  it('treats ? as a symbol regardless of shift', () => {
    expect(eventToSpec(ev('?', { shiftKey: true }))).toBe('?')
  })
})

describe('normalizeChord', () => {
  it('canonicalises modifier order and case', () => {
    expect(normalizeChord('Shift+Mod+Z')).toBe('mod+shift+z')
    expect(normalizeChord('alt+mod+x')).toBe('mod+alt+x')
    expect(normalizeChord(' ArrowLeft ')).toBe('arrowleft')
    expect(normalizeChord('F5')).toBe('f5')
  })
  it('drops shift for printable symbols, like eventToSpec', () => {
    expect(normalizeChord('shift+?')).toBe('?')
    expect(normalizeChord('shift+3')).toBe('shift+3')
  })
  it('rejects invalid chords', () => {
    for (const bad of ['', '+', 'mod+', 'mod+mod+a', 'ctrl+a', 'ab', 'f13', 'mod+shift', 'a+b', 'shift+shift+a'])
      expect(normalizeChord(bad), bad).toBeNull()
  })
  it('accepts every default binding as it is (the registry is canonical)', () => {
    for (const b of BINDINGS) for (const k of b.keys) expect(normalizeChord(k), `${b.id} ${k}`).toBe(k)
  })
  it('round-trips what eventToSpec produces', () => {
    for (const e of [ev('Z', { ctrlKey: true, shiftKey: true }), ev('ArrowUp'), ev(' ', { code: 'Space' }), ev('?', { shiftKey: true }), ev('F2')]) {
      const spec = eventToSpec(e)
      expect(normalizeChord(spec)).toBe(spec)
    }
  })
})

describe('specsFor', () => {
  it('lists every key of an action including hidden aliases', () => {
    expect(specsFor('edit.redo')).toEqual(['mod+shift+z', 'mod+y'])
  })
})

describe('dispatchKey', () => {
  it('runs the handler of the matching binding in scope', () => {
    const rate3 = vi.fn()
    const handlers: Handlers = { 'rate.3': rate3 }
    expect(dispatchKey(ev('3', { code: 'Digit3' }), 'grid', handlers)).toBe(true)
    expect(rate3).toHaveBeenCalledOnce()
  })

  it('distinguishes rate and rate-and-advance', () => {
    const rate = vi.fn()
    const next = vi.fn()
    dispatchKey(ev('#', { code: 'Digit3', shiftKey: true }), 'loupe', { 'rate.3': rate, 'rateNext.3': next })
    expect(rate).not.toHaveBeenCalled()
    expect(next).toHaveBeenCalledOnce()
  })

  it('respects scopes: Tab swaps in compare but toggles the sidebar elsewhere', () => {
    const swap = vi.fn()
    const sidebar = vi.fn()
    const handlers: Handlers = { 'compare.swap': swap, 'view.sidebar': sidebar }
    dispatchKey(ev('Tab'), 'compare', handlers)
    dispatchKey(ev('Tab'), 'grid', handlers)
    expect(swap).toHaveBeenCalledOnce()
    expect(sidebar).toHaveBeenCalledOnce()
  })

  it('maps colour keys 6-9 and flag keys', () => {
    expect(findBinding('6', 'grid')?.id).toBe('color.red')
    expect(findBinding('9', 'grid')?.id).toBe('color.blue')
    expect(findBinding('p', 'grid')?.id).toBe('flag.pick')
    expect(findBinding('x', 'grid')?.id).toBe('flag.reject')
    expect(findBinding('u', 'grid')?.id).toBe('flag.none')
  })

  it('maps undo / redo (both Ctrl+Shift+Z and Ctrl+Y)', () => {
    expect(findBinding('mod+z', 'grid')?.id).toBe('edit.undo')
    expect(findBinding('mod+shift+z', 'grid')?.id).toBe('edit.redo')
    expect(findBinding('mod+y', 'loupe')?.id).toBe('edit.redo')
    expect(findBinding('mod+e', 'compare')?.id).toBe('export.open')
  })

  it('returns false for unbound keys or missing handlers', () => {
    expect(dispatchKey(ev('q'), 'grid', {})).toBe(false)
    expect(dispatchKey(ev('p'), 'grid', {})).toBe(false)
  })

  it('does not dispatch hold-style bindings (Z handled separately)', () => {
    expect(findBinding('z', 'loupe')).toBeUndefined()
  })

  it('selects all only in grid', () => {
    expect(findBinding('mod+a', 'grid')?.id).toBe('select.all')
    expect(findBinding('mod+a', 'loupe')).toBeUndefined()
  })
})

describe('registry', () => {
  it('has no duplicate key within the same scope', () => {
    const seen = new Map<string, string>()
    for (const b of BINDINGS) {
      if (b.hold) continue
      for (const scope of b.scopes)
        for (const k of b.keys) {
          const id = `${scope}:${k}`
          // aliases of the same action are allowed
          expect(seen.get(id) ?? b.id).toBe(b.id)
          seen.set(id, b.id)
        }
    }
  })

  it('generates help groups including the ? shortcut and collapsed rating rows', () => {
    const groups = helpGroups()
    const all = groups.flatMap((g) => g.items)
    expect(all.some((i) => i.desc === 'help')).toBe(true)
    expect(all.filter((i) => i.desc === 'rate')).toHaveLength(1)
    expect(all.find((i) => i.desc === 'zoomHold')).toBeTruthy()
  })
})
