import { describe, expect, it } from 'vitest'
import { foldedAway, groupViewState } from './counts'

describe('groupViewState', () => {
  it('opens for a real burst', () => {
    expect(groupViewState({ analyzed: true, burst_id: 3, burst_size: 4 })).toBe('ok')
  })
  it('asks for analysis only when the photo was not analysed', () => {
    expect(groupViewState({ analyzed: false, burst_id: null, burst_size: null })).toBe('not-analysed')
  })
  it('says "no similar shots" for an analysed single', () => {
    expect(groupViewState({ analyzed: true, burst_id: 7, burst_size: 1 })).toBe('no-similar')
    expect(groupViewState({ analyzed: true, burst_id: null, burst_size: null })).toBe('no-similar')
  })
})

describe('foldedAway', () => {
  it('counts photos hidden in collapsed stacks', () => {
    expect(foldedAway(1000, 692)).toBe(308)
    expect(foldedAway(10, 10)).toBe(0)
    expect(foldedAway(5, 9)).toBe(0)
  })
})
