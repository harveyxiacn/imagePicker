import { describe, expect, it } from 'vitest'
import { overlayOwnsEscape } from './panelOverlay'

describe('overlayOwnsEscape', () => {
  const base = { panelOpen: true, typing: false, dialogOpen: false, menuOpen: false }
  it('takes Escape when a floating panel is open and nothing else wants it', () => {
    expect(overlayOwnsEscape(base)).toBe(true)
  })
  it('leaves Escape to nav.back when no panel is open', () => {
    expect(overlayOwnsEscape({ ...base, panelOpen: false })).toBe(false)
  })
  it('leaves Escape to text fields, dialogs and menus', () => {
    expect(overlayOwnsEscape({ ...base, typing: true })).toBe(false)
    expect(overlayOwnsEscape({ ...base, dialogOpen: true })).toBe(false)
    expect(overlayOwnsEscape({ ...base, menuOpen: true })).toBe(false)
  })
})
