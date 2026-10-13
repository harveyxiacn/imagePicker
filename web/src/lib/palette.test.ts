import { describe, expect, it } from 'vitest'
import { BINDINGS, findBinding } from './keymap'
import { actionItems, buildItems, fuzzyScore, navTargets, paletteSpecs, rankItems, scoreItem } from './palette'

// identity-ish translator: `keys.undo` -> "Undo", `keyGroup.edit` -> "Edit"
const t = (k: string) => {
  const last = k.split('.').pop() ?? k
  return last.charAt(0).toUpperCase() + last.slice(1)
}
const all = () => true

describe('fuzzyScore', () => {
  it('rejects text that does not contain the characters in order', () => {
    expect(fuzzyScore('xyz', 'Undo')).toBeNull()
    expect(fuzzyScore('odnu', 'Undo')).toBeNull()
  })
  it('ranks prefix > word start > inner substring > scattered subsequence', () => {
    const prefix = fuzzyScore('und', 'Undo')!
    const word = fuzzyScore('fit', 'Zoom fit')!
    const inner = fuzzyScore('ndo', 'Undo')!
    const scattered = fuzzyScore('ud', 'Undo')!
    expect(prefix).toBeGreaterThan(word)
    expect(word).toBeGreaterThan(inner)
    expect(inner).toBeGreaterThan(scattered)
  })
  it('is case-insensitive and handles CJK substrings', () => {
    expect(fuzzyScore('UNDO', 'undo')).not.toBeNull()
    expect(fuzzyScore('撤销', '撤销上一步')).toBeGreaterThan(fuzzyScore('上一', '撤销上一步')!)
  })
  it('an empty token matches everything', () => {
    expect(fuzzyScore('', 'anything')).toBe(0)
  })
})

describe('scoreItem / rankItems', () => {
  const items = [
    { label: 'Export', group: 'Other', extra: 'export open' },
    { label: 'Expand all stacks', group: 'AI', extra: 'stacks toggleAll' },
    { label: 'Undo', group: 'Edit', extra: 'edit undo' },
    { label: 'Redo', group: 'Edit', extra: 'edit redo' },
  ]
  it('keeps the original order for an empty query', () => {
    expect(rankItems(items, '  ').map((i) => i.label)).toEqual(['Export', 'Expand all stacks', 'Undo', 'Redo'])
  })
  it('puts the better match first and drops non-matches', () => {
    expect(rankItems(items, 'exp').map((i) => i.label)).toEqual(['Export', 'Expand all stacks'])
    expect(rankItems(items, 'undo')[0].label).toBe('Undo')
    expect(rankItems(items, 'zzz')).toEqual([])
  })
  it('matches the action id words and the group heading as a fallback', () => {
    expect(rankItems(items, 'toggleall')[0].label).toBe('Expand all stacks')
    expect(rankItems(items, 'edit').map((i) => i.label)).toContain('Undo')
  })
  it('requires every whitespace-separated token', () => {
    expect(scoreItem(items[2], 'edit undo')).not.toBeNull()
    expect(scoreItem(items[2], 'edit redo')).toBeNull()
  })
  it('honours the limit and is stable for ties', () => {
    const many = Array.from({ length: 10 }, (_, i) => ({ label: `Item ${i}`, group: '', extra: '' }))
    expect(rankItems(many, 'item', 3).map((i) => i.label)).toEqual(['Item 0', 'Item 1', 'Item 2'])
  })
})

describe('actionItems (generated from the keymap)', () => {
  it('lists each action id once, only for the scope, and never the palette itself', () => {
    const grid = actionItems({ scope: 'grid', t, available: all })
    const ids = grid.map((i) => i.id)
    expect(new Set(ids).size).toBe(ids.length)
    expect(ids).toContain('select.all')
    expect(ids).not.toContain('palette.toggle')
    expect(ids).not.toContain('zoom.hold') // hold-style
    const loupe = actionItems({ scope: 'loupe', t, available: all }).map((i) => i.id)
    expect(loupe).not.toContain('select.all')
    expect(actionItems({ scope: 'edit', t, available: all }).map((i) => i.id)).toContain('edit.before')
  })
  it('only offers actions the screen can run', () => {
    const items = actionItems({ scope: 'grid', t, available: (id) => id === 'flag.pick' })
    expect(items.map((i) => i.id)).toEqual(['flag.pick'])
  })
  it('merges alias keys and hides hidden-only bindings from the key caps', () => {
    const redo = actionItems({ scope: 'grid', t, available: all }).find((i) => i.id === 'edit.redo')!
    expect(redo.keys).toHaveLength(1) // mod+shift+z; the mod+y alias is hidden
    expect(redo.keys[0]).toContain('Z')
  })
  it('numbers the rating actions', () => {
    const items = actionItems({ scope: 'grid', t, available: all })
    expect(items.find((i) => i.id === 'rate.3')?.label).toBe('Rate 3')
    expect(items.find((i) => i.id === 'rateNext.5')?.label).toBe('RateNext 5')
  })
  it('finds an action by typing its name', () => {
    const items = buildItems({ scope: 'grid', t, available: all, sessionId: 7 })
    expect(rankItems(items, 'undo')[0].id).toBe('edit.undo')
    expect(rankItems(items, 'pick')[0].id).toBe('flag.pick')
  })
})

describe('navigation targets', () => {
  it('offers session pages only inside a library', () => {
    expect(navTargets(null).map((n) => n.id)).toEqual(['goto.home', 'goto.settings', 'goto.taste'])
    const withSession = navTargets(12)
    expect(withSession.find((n) => n.id === 'goto.people')?.path).toBe('/s/12/people')
    expect(withSession.find((n) => n.id === 'goto.library')?.path).toBe('/s/12')
  })
  it('buildItems puts actions before navigation', () => {
    const items = buildItems({ scope: 'grid', t, available: all, sessionId: 1 })
    const firstNav = items.findIndex((i) => i.kind === 'nav')
    expect(firstNav).toBeGreaterThan(0)
    expect(items.slice(firstNav).every((i) => i.kind === 'nav')).toBe(true)
  })
})

describe('keymap integration', () => {
  it('Ctrl+K is registered once in the keymap and resolves in every scope', () => {
    expect(paletteSpecs()).toEqual(['mod+k'])
    for (const scope of ['grid', 'loupe', 'compare', 'group', 'edit'] as const) {
      expect(findBinding('mod+k', scope)?.id).toBe('palette.toggle')
    }
    expect(BINDINGS.filter((b) => b.id === 'palette.toggle')).toHaveLength(1)
  })
})
