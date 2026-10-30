import { describe, expect, it } from 'vitest'
import { keyboardInset, menuPortalRoot, rovingIndex, tabIndexIn } from './popover'

describe('rovingIndex', () => {
  it('moves down and up with wrap-around', () => {
    expect(rovingIndex(0, 3, 'ArrowDown')).toBe(1)
    expect(rovingIndex(2, 3, 'ArrowDown')).toBe(0)
    expect(rovingIndex(0, 3, 'ArrowUp')).toBe(2)
    expect(rovingIndex(2, 3, 'ArrowUp')).toBe(1)
  })
  it('starts from the ends when nothing is focused', () => {
    expect(rovingIndex(-1, 4, 'ArrowDown')).toBe(0)
    expect(rovingIndex(-1, 4, 'ArrowUp')).toBe(3)
  })
  it('Home / End jump to the ends', () => {
    expect(rovingIndex(2, 5, 'Home')).toBe(0)
    expect(rovingIndex(1, 5, 'End')).toBe(4)
  })
  it('ignores other keys and empty menus', () => {
    expect(rovingIndex(0, 3, 'a')).toBeNull()
    expect(rovingIndex(0, 3, 'Enter')).toBeNull()
    expect(rovingIndex(-1, 0, 'ArrowDown')).toBeNull()
  })
})

describe('tabIndexIn', () => {
  it('cycles forward and backward', () => {
    expect(tabIndexIn(0, 3, false)).toBe(1)
    expect(tabIndexIn(2, 3, false)).toBe(0)
    expect(tabIndexIn(0, 3, true)).toBe(2)
    expect(tabIndexIn(2, 3, true)).toBe(1)
  })
  it('enters from outside at the first / last element', () => {
    expect(tabIndexIn(-1, 3, false)).toBe(0)
    expect(tabIndexIn(-1, 3, true)).toBe(2)
    expect(tabIndexIn(-1, 0, false)).toBe(-1)
  })
})

describe('keyboardInset', () => {
  it('is the layout height the visual viewport leaves uncovered at the bottom', () => {
    expect(keyboardInset(800, { height: 500, offsetTop: 0 })).toBe(300)
    expect(keyboardInset(800, { height: 450, offsetTop: 50 })).toBe(300)
  })
  it('is zero without a keyboard, noise, or visualViewport', () => {
    expect(keyboardInset(800, { height: 800, offsetTop: 0 })).toBe(0)
    expect(keyboardInset(800, { height: 770, offsetTop: 0 })).toBe(0)
    expect(keyboardInset(800, null)).toBe(0)
  })
})

describe('menuPortalRoot', () => {
  const body = { tag: 'body' } as unknown as HTMLElement
  const main = { tag: 'main' } as unknown as HTMLElement
  it('portals into the main landmark when the page has one', () => {
    expect(menuPortalRoot({ querySelector: () => main, body } as unknown as Document)).toBe(main)
  })
  it('falls back to body without a main landmark', () => {
    expect(menuPortalRoot({ querySelector: () => null, body } as unknown as Document)).toBe(body)
  })
})
