import { describe, expect, it } from 'vitest'
import type { Photo } from '@/api/types'
import { buildDeck, currentCard, deckProgress, deckReducer, decisionFor, initDeck, isDone, stackBehind } from './quickCull'

const photo = (id: number, burst: number | null, rank: number | null, score: number | null = null): Photo =>
  ({ id, burst_id: burst, rank_in_burst: rank, ai_score: score }) as Photo

const PHOTOS = [
  photo(1, 10, 3, 0.2),
  photo(2, 10, 0, 0.9),
  photo(3, 10, 1, 0.8),
  photo(4, 10, 2, 0.5),
  photo(5, null, null),
  photo(6, 11, 1, 0.4),
  photo(7, 11, 0, 0.7),
  photo(8, 12, null, null), // singleton burst: not a group
]

describe('buildDeck', () => {
  it('keeps only bursts with 2+ photos, top 3 by rank, in library order', () => {
    expect(buildDeck(PHOTOS)).toEqual([
      { id: 10, cards: [2, 3, 4] },
      { id: 11, cards: [7, 6] },
    ])
  })

  it('falls back to AI score then id when ranks are missing', () => {
    const d = buildDeck([photo(1, 1, null, 0.3), photo(2, 1, null, 0.9), photo(3, 1, null, 0.9), photo(4, 1, null, 0.1)])
    expect(d[0].cards).toEqual([2, 3, 1])
  })

  it('respects a custom deck size and handles no bursts', () => {
    expect(buildDeck(PHOTOS, 2)[0].cards).toEqual([2, 3])
    expect(buildDeck([photo(1, null, null)])).toEqual([])
  })
})

describe('deckReducer', () => {
  const start = () => initDeck(buildDeck(PHOTOS))

  it('starts on the best card of the first group', () => {
    const s = start()
    expect(currentCard(s)).toBe(2)
    expect(stackBehind(s)).toEqual([3, 4])
    expect(deckProgress(s)).toMatchObject({ group: 1, groups: 2, card: 1, cards: 3, decided: 0, total: 5 })
  })

  it('records decisions and advances within then across groups', () => {
    let s = start()
    s = deckReducer(s, { type: 'decide', decision: decisionFor('right') })
    expect(s.decisions[2]).toBe('keep')
    expect(currentCard(s)).toBe(3)
    s = deckReducer(s, { type: 'decide', decision: decisionFor('left') })
    s = deckReducer(s, { type: 'decide', decision: 'keep' })
    expect(s.decisions).toEqual({ 2: 'keep', 3: 'reject', 4: 'keep' })
    expect(deckProgress(s)).toMatchObject({ group: 2, card: 1, cards: 2, decided: 3 })
    expect(currentCard(s)).toBe(7)
  })

  it('finishes and ignores further decisions', () => {
    let s = start()
    for (let i = 0; i < 5; i++) s = deckReducer(s, { type: 'decide', decision: 'keep' })
    expect(isDone(s)).toBe(true)
    expect(currentCard(s)).toBeNull()
    const again = deckReducer(s, { type: 'decide', decision: 'keep' })
    expect(again).toBe(s)
    expect(deckProgress(s)).toMatchObject({ group: 2, groups: 2, decided: 5, total: 5 })
  })

  it('undo restores the previous card, including across a group boundary', () => {
    let s = start()
    for (let i = 0; i < 4; i++) s = deckReducer(s, { type: 'decide', decision: 'keep' })
    expect(currentCard(s)).toBe(6)
    s = deckReducer(s, { type: 'undo' })
    expect(currentCard(s)).toBe(7)
    expect(s.decisions[7]).toBeUndefined()
    s = deckReducer(s, { type: 'undo' })
    expect(currentCard(s)).toBe(4)
    expect(s.gi).toBe(0)
    expect(deckReducer(start(), { type: 'undo' }).history).toHaveLength(0)
  })

  it('skipGroup / goGroup move between groups without deciding', () => {
    let s = start()
    s = deckReducer(s, { type: 'skipGroup' })
    expect(s.gi).toBe(1)
    expect(Object.keys(s.decisions)).toHaveLength(0)
    s = deckReducer(s, { type: 'goGroup', delta: -1 })
    expect(s.gi).toBe(0)
    s = deckReducer(s, { type: 'goGroup', delta: -1 })
    expect(s.gi).toBe(0)
    s = deckReducer(s, { type: 'skipGroup' })
    s = deckReducer(s, { type: 'skipGroup' })
    expect(isDone(s)).toBe(true)
    expect(deckReducer(s, { type: 'skipGroup' })).toBe(s)
  })
})
