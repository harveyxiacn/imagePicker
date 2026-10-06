import { describe, expect, it } from 'vitest'
import { activeNavTab, isMobileWidth, layoutClass, navTarget, sessionFromPath, settleSheet, sheetHeight, showsBottomNav } from './layout'

describe('layout class', () => {
  it('splits at 768 and 1024', () => {
    expect(layoutClass(412)).toBe('mobile')
    expect(layoutClass(767)).toBe('mobile')
    expect(layoutClass(768)).toBe('tablet')
    expect(layoutClass(1023)).toBe('tablet')
    expect(layoutClass(1024)).toBe('desktop')
    expect(isMobileWidth(767)).toBe(true)
    expect(isMobileWidth(768)).toBe(false)
  })
})

describe('bottom navigation', () => {
  it('hides on full-screen flows', () => {
    expect(showsBottomNav('/')).toBe(true)
    expect(showsBottomNav('/s/3')).toBe(true)
    expect(showsBottomNav('/settings')).toBe(true)
    expect(showsBottomNav('/s/3/people')).toBe(true)
    expect(showsBottomNav('/s/3/edit/44')).toBe(false)
    expect(showsBottomNav('/s/3/besttake/9')).toBe(false)
    expect(showsBottomNav('/login')).toBe(false)
  })

  it('highlights the right tab', () => {
    expect(activeNavTab('/', 'grid')).toBe('library')
    expect(activeNavTab('/s/3', 'grid')).toBe('library')
    expect(activeNavTab('/s/3', 'loupe')).toBe('cull')
    expect(activeNavTab('/s/3', 'group')).toBe('cull')
    expect(activeNavTab('/s/3/people', 'loupe')).toBe('people')
    expect(activeNavTab('/settings', 'grid')).toBe('settings')
  })

  it('needs a session for cull / people', () => {
    expect(navTarget('library', null)).toBe('/')
    expect(navTarget('cull', null)).toBeNull()
    expect(navTarget('people', null)).toBeNull()
    expect(navTarget('settings', null)).toBe('/settings')
    expect(navTarget('cull', 7)).toBe('/s/7')
    expect(navTarget('people', 7)).toBe('/s/7/people')
    expect(sessionFromPath('/s/12/people')).toBe(12)
    expect(sessionFromPath('/settings')).toBeNull()
  })
})

describe('bottom sheets', () => {
  it('clamps height to the viewport', () => {
    expect(sheetHeight(800, 2000)).toBe(720)
    expect(sheetHeight(800, 50)).toBe(160)
    expect(sheetHeight(800, 400)).toBe(400)
  })

  it('settles a drag', () => {
    expect(settleSheet(300, 0, 600)).toBe('close')
    expect(settleSheet(20, 0.9, 600)).toBe('close')
    expect(settleSheet(40, 0.1, 600)).toBe('stay')
    expect(settleSheet(-80, 0, 600)).toBe('expand')
    expect(settleSheet(-10, -0.8, 600)).toBe('expand')
  })
})
