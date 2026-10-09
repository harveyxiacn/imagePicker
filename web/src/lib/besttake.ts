/**
 * View-model of a Best Take plan (docs/api-contract-m5.md C, doc 04 section 3.4): turns the plan returned by
 * `GET /api/bursts/{id}/besttake` plus the base photo's edit stack into what the editor shows.
 */
import type { BestTakeChoice, BestTakeCandidate, BestTakePerson, BestTakePlan, BestTakeWarning, EditStack } from '@/api/types'
import { bestTakeOf, isPatchEnabled } from './patches'

export interface CandidateView extends BestTakeCandidate {
  /** 1-based position in shot order (the filmstrip / matrix numbering), null when unknown */
  frame: number | null
  /** the candidate is the base photo itself (= keep the original face) */
  isBase: boolean
  /** highest-scoring composable candidate of the person */
  isBest: boolean
  /** the face currently pasted into the base (stack patch) */
  isCurrent: boolean
  /** expression score as 0..100 */
  score: number
}

export interface PersonView {
  key: number
  track_id: number
  person_id: number | null
  name: string | null
  /** this person's face id in the chosen base photo */
  baseFaceId: number
  candidates: CandidateView[]
  /** highest-scoring composable candidate (may be the base photo) */
  best: CandidateView | undefined
  /** best composable candidate that is NOT the base and improves on it by more than MIN_GAIN, i.e. what "best for everyone" would paste */
  bestSwap: CandidateView | undefined
  current: CandidateView | undefined
  replaced: boolean
}

export interface PlanViewOptions {
  /** chosen base photo (defaults to the plan's base) */
  baseId?: number
  /** photo ids in shot order, for the `#n` numbering */
  order?: number[]
  /** current stack of the base photo (to find the layers already applied) */
  stack?: EditStack
  /** face id of a track in another photo (burst faces matrix); used when the base is not the plan's base */
  faceIdOf?: (trackId: number, photoId: number) => number | undefined
}

/** A face is only worth replacing when a candidate beats it by more than this (core `MIN_GAIN`). */
export const MIN_GAIN = 0.04

export const scoreOf = (c: Pick<BestTakeCandidate, 'expression_score'>): number => Math.round(Math.max(0, Math.min(1, c.expression_score)) * 100)

const byScoreDesc = (a: BestTakeCandidate, b: BestTakeCandidate) => b.expression_score - a.expression_score || a.photo_id - b.photo_id

/** Face id of `person` in `baseId`: from the candidate list when it contains the frame, else through `faceIdOf`. */
export function baseFaceIdFor(person: BestTakePerson, plan: BestTakePlan, baseId: number, faceIdOf?: PlanViewOptions['faceIdOf']): number {
  if (baseId === plan.base_photo_id) return person.base_face_id
  return person.candidates.find((c) => c.photo_id === baseId)?.face_id ?? faceIdOf?.(person.track_id, baseId) ?? person.base_face_id
}

export function buildPlanView(plan: BestTakePlan, opts: PlanViewOptions = {}): PersonView[] {
  const baseId = opts.baseId ?? plan.base_photo_id
  const order = opts.order ?? []
  return plan.people.map((p) => {
    // the base frame itself is always usable (choosing it restores the original face), whatever the plan says
    const sorted = p.candidates.map((c) => (c.photo_id === baseId ? { ...c, composable: true, reason: null } : c)).sort(byScoreDesc)
    const applied = opts.stack && p.person_id !== null ? bestTakeOf(opts.stack, p.person_id) : undefined
    const currentPhoto = applied && isPatchEnabled(applied) ? applied.source_photo_id : undefined
    const bestComposable = sorted.find((c) => c.composable)
    const base = sorted.find((c) => c.photo_id === baseId)
    const candidates: CandidateView[] = sorted.map((c) => ({
      ...c,
      frame: order.indexOf(c.photo_id) >= 0 ? order.indexOf(c.photo_id) + 1 : null,
      isBase: c.photo_id === baseId,
      isBest: c === bestComposable,
      isCurrent: currentPhoto !== undefined && c.photo_id === currentPhoto,
      score: scoreOf(c),
    }))
    const bestSwap = candidates.find((c) => c.composable && !c.isBase && (!base || c.expression_score > base.expression_score + MIN_GAIN))
    const current = candidates.find((c) => c.isCurrent)
    return {
      key: p.track_id,
      track_id: p.track_id,
      person_id: p.person_id,
      name: p.person_name,
      baseFaceId: baseFaceIdFor(p, plan, baseId, opts.faceIdOf),
      candidates,
      best: candidates.find((c) => c.isBest),
      bestSwap,
      current,
      replaced: current !== undefined,
    }
  })
}

/** The choice that pastes `candidate`'s face into `person`'s face of the base (POST /api/besttake body item). */
export const choiceFor = (person: PersonView, candidate: Pick<BestTakeCandidate, 'photo_id' | 'face_id'>): BestTakeChoice => ({
  base_face_id: person.baseFaceId,
  source_photo_id: candidate.photo_id,
  source_face_id: candidate.face_id,
})

/** Client-side "best for everyone": every person whose best composable candidate beats the base gets a choice. */
export const autoChoices = (view: PersonView[]): BestTakeChoice[] => view.flatMap((p) => (p.bestSwap ? [choiceFor(p, p.bestSwap)] : []))

/** People that already are at their best in the base (nothing to composite). */
export const alreadyBest = (view: PersonView[]): PersonView[] => view.filter((p) => !p.bestSwap)

/** Candidates shown by default: the top `limit` by score plus the applied one; `all` shows every frame. */
export function visibleCandidates(person: PersonView, all: boolean, limit = 3): CandidateView[] {
  if (all || person.candidates.length <= limit) return person.candidates
  const top = person.candidates.slice(0, limit)
  const cur = person.candidates.find((c) => c.isCurrent && !top.includes(c))
  return cur ? [...top, cur] : top
}

/** Warnings of a result, de-duplicated and ordered for display. */
export const WARNING_ORDER: BestTakeWarning[] = ['large_pose_change', 'camera_moved', 'occlusion', 'seam']
export function sortedWarnings(w: readonly BestTakeWarning[] | undefined): BestTakeWarning[] {
  const set = new Set(w ?? [])
  return WARNING_ORDER.filter((k) => set.has(k))
}

/** Face boxes need the frame number of a photo for the title ("底片 #3"). */
export const frameNumber = (order: number[], photoId: number): number | null => (order.indexOf(photoId) >= 0 ? order.indexOf(photoId) + 1 : null)

/** Why the plan picked its base: i18n key (`besttake.why_*`) and params; undefined when the server sends no `base_choice`. */
export function baseWhy(plan: BestTakePlan, order: number[]): { key: string; params: { n: number; m: number; frame: number | string } } | undefined {
  const c = plan.base_choice
  if (!c) return undefined
  const work = (id: number) => c.frames.find((f) => f.photo_id === id)?.replacements ?? 0
  return { key: `besttake.why_${c.reason}`, params: { n: work(plan.base_photo_id), m: work(c.group_best_photo_id), frame: frameNumber(order, c.group_best_photo_id) ?? '?' } }
}

/** Faces "best for everyone" would replace if `photoId` were the base (from the plan's `base_choice`). */
export const frameWork = (plan: BestTakePlan | undefined, photoId: number): number | undefined => plan?.base_choice?.frames.find((f) => f.photo_id === photoId)?.replacements
