import type { Photo } from '@/api/types'

/** Quick cull (doc 08 section 5): per group only the AI top-N cards, Tinder-style swipes. */

export const DECK_SIZE = 3

export interface DeckGroup {
  id: number
  /** Photo ids, best first. */
  cards: number[]
}

export type Decision = 'keep' | 'reject'

export interface DeckState {
  groups: DeckGroup[]
  gi: number
  ci: number
  decisions: Record<number, Decision>
  /** Decided cards in order (for undo). */
  history: { gi: number; ci: number; id: number }[]
}

export type DeckAction =
  | { type: 'decide'; decision: Decision }
  | { type: 'undo' }
  | { type: 'skipGroup' }
  | { type: 'goGroup'; delta: 1 | -1 }

/** Sort: burst rank first (0 = best), then AI score (higher first), then id for stability. */
function better(a: Photo, b: Photo): number {
  const ra = a.rank_in_burst ?? Infinity
  const rb = b.rank_in_burst ?? Infinity
  if (ra !== rb) return ra - rb
  const sa = a.ai_score ?? -1
  const sb = b.ai_score ?? -1
  if (sa !== sb) return sb - sa
  return a.id - b.id
}

/** Groups = bursts with 2+ photos, in library order; each keeps its top `n` photos. */
export function buildDeck(photos: Photo[], n = DECK_SIZE): DeckGroup[] {
  const order: number[] = []
  const byBurst = new Map<number, Photo[]>()
  for (const p of photos) {
    if (p.burst_id === null) continue
    let list = byBurst.get(p.burst_id)
    if (!list) {
      list = []
      byBurst.set(p.burst_id, list)
      order.push(p.burst_id)
    }
    list.push(p)
  }
  return order
    .map((id) => ({ id, list: byBurst.get(id)! }))
    .filter((g) => g.list.length >= 2)
    .map((g) => ({ id: g.id, cards: [...g.list].sort(better).slice(0, n).map((p) => p.id) }))
}

export function initDeck(groups: DeckGroup[]): DeckState {
  return { groups, gi: 0, ci: 0, decisions: {}, history: [] }
}

export const isDone = (s: DeckState): boolean => s.gi >= s.groups.length

export function currentCard(s: DeckState): number | null {
  return s.groups[s.gi]?.cards[s.ci] ?? null
}

/** Cards behind the current one in the same group (rendered as the stack). */
export function stackBehind(s: DeckState): number[] {
  const g = s.groups[s.gi]
  return g ? g.cards.slice(s.ci + 1) : []
}

export interface DeckProgress {
  group: number
  groups: number
  card: number
  cards: number
  /** Decided cards over all cards of all groups. */
  decided: number
  total: number
}

export function deckProgress(s: DeckState): DeckProgress {
  const g = s.groups[Math.min(s.gi, s.groups.length - 1)]
  return {
    group: Math.min(s.gi + 1, s.groups.length),
    groups: s.groups.length,
    card: isDone(s) ? (g?.cards.length ?? 0) : s.ci + 1,
    cards: g?.cards.length ?? 0,
    decided: Object.keys(s.decisions).length,
    total: s.groups.reduce((n, x) => n + x.cards.length, 0),
  }
}

export function deckReducer(s: DeckState, a: DeckAction): DeckState {
  switch (a.type) {
    case 'decide': {
      const id = currentCard(s)
      if (id === null) return s
      let gi = s.gi
      let ci = s.ci + 1
      if (ci >= s.groups[gi].cards.length) {
        gi += 1
        ci = 0
      }
      return { ...s, gi, ci, decisions: { ...s.decisions, [id]: a.decision }, history: [...s.history, { gi: s.gi, ci: s.ci, id }] }
    }
    case 'undo': {
      const last = s.history[s.history.length - 1]
      if (!last) return s
      const decisions = { ...s.decisions }
      delete decisions[last.id]
      return { ...s, gi: last.gi, ci: last.ci, decisions, history: s.history.slice(0, -1) }
    }
    case 'skipGroup':
      return isDone(s) ? s : { ...s, gi: s.gi + 1, ci: 0 }
    case 'goGroup': {
      const gi = Math.min(s.groups.length, Math.max(0, s.gi + a.delta))
      return gi === s.gi ? s : { ...s, gi, ci: 0 }
    }
  }
}

/** Swipe direction to decision (right keeps, left rejects). */
export function decisionFor(dir: 'left' | 'right'): Decision {
  return dir === 'right' ? 'keep' : 'reject'
}
