import type { Issue, Photo } from '@/api/types'
import { computeChanges, type Change } from './history'

export interface IssueBadge {
  issue: Issue
  /** Glyph shown on the thumbnail (doc 04 3.2). */
  glyph: string
  /** i18n key under `issue.` */
  label: string
  /** Tailwind text colour class */
  tone: string
}

const BADGES: Record<Issue, IssueBadge> = {
  closed_eyes: { issue: 'closed_eyes', glyph: '⚠', label: 'closed_eyes', tone: 'text-warning' },
  blurry: { issue: 'blurry', glyph: '🌫', label: 'blurry', tone: 'text-white' },
  overexposed: { issue: 'overexposed', glyph: '☀', label: 'overexposed', tone: 'text-warning' },
  underexposed: { issue: 'underexposed', glyph: '🌑', label: 'underexposed', tone: 'text-white' },
  noisy: { issue: 'noisy', glyph: '▒', label: 'noisy', tone: 'text-white' },
  tilted: { issue: 'tilted', glyph: '⟋', label: 'tilted', tone: 'text-white' },
}

/** Display order: the most actionable problem first. */
const ORDER: Issue[] = ['closed_eyes', 'blurry', 'overexposed', 'underexposed', 'noisy', 'tilted']

/** Map issue tags to ordered thumbnail badges; unknown tags (newer backend) are dropped. */
export function issueBadges(issues: readonly Issue[] | undefined): IssueBadge[] {
  if (!issues?.length) return []
  const set = new Set(issues)
  return ORDER.filter((i) => set.has(i)).map((i) => BADGES[i])
}

/** `user_rating = round(ai_rating)` (contract C.2); half stars round up like Rust's f64::round. */
export const acceptedRating = (ai: number): number => Math.min(5, Math.max(0, Math.round(ai)))

/** Photos that can take an AI rating (analysed, AI rating present). */
export const canAccept = (p: Pick<Photo, 'analyzed' | 'ai_rating'>): boolean => p.analyzed && p.ai_rating !== null

/**
 * Undo-able changes for "accept AI rating": one Change per photo whose user rating would actually change.
 * Grouping into PATCH requests happens via `groupPatches(changes, side)` (same as M1 edits).
 */
export function planAcceptAi(photos: Pick<Photo, 'id' | 'analyzed' | 'ai_rating' | 'user_rating' | 'flag' | 'color_label'>[]): Change[] {
  const changes: Change[] = []
  for (const p of photos) {
    if (!canAccept(p)) continue
    changes.push(...computeChanges([p], { user_rating: acceptedRating(p.ai_rating!) }))
  }
  return changes
}

/** Star presentation for an AI rating in 0.5 steps: 'full' | 'half' | 'empty' per star. */
export function starFills(value: number | null): ('full' | 'half' | 'empty')[] {
  const v = value ?? 0
  return [1, 2, 3, 4, 5].map((n) => (v >= n ? 'full' : v >= n - 0.5 ? 'half' : 'empty'))
}

/** Expression colour band for the person x frame matrix (doc 04 3.3): green open+smile, red closed/blurry, else yellow. */
export type CellTone = 'good' | 'ok' | 'bad' | 'none'
export function expressionTone(face: { eyes_open: number | null; smile: number | null; sharpness: number | null } | null): CellTone {
  if (!face) return 'none'
  const eyes = face.eyes_open ?? 1
  const sharp = face.sharpness ?? 1
  if (eyes < 0.45 || sharp < 0.25) return 'bad'
  if (eyes >= 0.6 && (face.smile ?? 0) >= 0.55) return 'good'
  return 'ok'
}

/** Frames that are in the top-3 of every present track; empty = no single frame is best for everyone. */
export function commonBestFrames(tracks: { best_photo_ids: number[] }[]): number[] {
  if (tracks.length === 0) return []
  const [first, ...rest] = tracks
  return first.best_photo_ids.filter((id) => rest.every((t) => t.best_photo_ids.includes(id)))
}

export const personLabel = (p: { id: number; name: string | null }, unnamed: string): string => p.name ?? `${unnamed} ${p.id}`
