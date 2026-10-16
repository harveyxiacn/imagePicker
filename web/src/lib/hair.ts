/**
 * Hair transfer ("换成我最好的发型", docs/api-contract-m5.md H): pure helpers for the Hair panel.
 * A transfer is a `patch` layer of kind `hair` per face (`base_face_id`); its `amount` is the strength.
 */
import type { EditStack, HairDonor, PatchOp, PhotoPerson } from '@/api/types'
import { patchEntries, removePatch, updatePatch } from './patches'

const hairEntry = (s: EditStack, faceId: number) => patchEntries(s).find((e) => e.op.kind === 'hair' && e.op.base_face_id === faceId)

/** The hair layer of a face, if any. */
export const hairLayerOf = (s: EditStack, faceId: number): PatchOp | undefined => hairEntry(s, faceId)?.op

/** Faces that have a hair layer. */
export const hairFaceIds = (s: EditStack): number[] =>
  patchEntries(s).flatMap((e) => (e.op.kind === 'hair' && e.op.base_face_id !== undefined ? [e.op.base_face_id] : []))

/** Change the strength (patch `amount`, 0..1) of a face's hair layer. */
export function setHairStrength(s: EditStack, faceId: number, amount: number): EditStack {
  const e = hairEntry(s, faceId)
  if (!e) return s
  const a = Math.round(Math.min(1, Math.max(0, amount)) * 100) / 100
  return updatePatch(s, e.index, (p) => ({ ...p, amount: a }))
}

/** Show / hide a face's hair layer (before / after without losing it). */
export function setHairEnabled(s: EditStack, faceId: number, enabled: boolean): EditStack {
  const e = hairEntry(s, faceId)
  return e ? updatePatch(s, e.index, (p) => ({ ...p, enabled })) : s
}

/** Drop a face's hair layer (back to the photo's own hair). */
export function removeHair(s: EditStack, faceId: number): EditStack {
  const e = hairEntry(s, faceId)
  return e ? removePatch(s, e.index) : s
}

/** The face the panel starts with: the largest subject face, else the largest face. */
export function defaultHairFace(people: PhotoPerson[]): number | undefined {
  const area = (p: PhotoPerson) => p.face_box[2] * p.face_box[3]
  const pool = people.some((p) => p.is_subject) ? people.filter((p) => p.is_subject) : people
  return [...pool].sort((a, b) => area(b) - area(a))[0]?.face_id
}

/** First composable donor (the server sorts: same session, composable, score). */
export const bestDonor = (donors: HairDonor[]): HairDonor | undefined => donors.find((d) => d.composable)

/** i18n keys explaining a donor: the refusal reason, or its cues (the ranking's "why"). */
export function donorReasonKeys(d: HairDonor): string[] {
  if (!d.composable) return [`hair.reason_${d.reason ?? 'unknown'}`]
  return d.cues.map((c) => `hair.cue_${c}`)
}

/** "2 h 05 min earlier" style offset parts, or null when either photo has no time. */
export function timeOffset(minutes: number | null): { key: 'hair.earlier' | 'hair.later' | 'hair.sameTime'; h: number; m: number } | null {
  if (minutes === null) return null
  if (Math.abs(minutes) < 1) return { key: 'hair.sameTime', h: 0, m: 0 }
  const a = Math.abs(minutes)
  return { key: minutes < 0 ? 'hair.earlier' : 'hair.later', h: Math.floor(a / 60), m: a % 60 }
}

/** Score as a whole percentage (null = not measured). */
export const scorePct = (s: number | null): number | null => (s === null ? null : Math.round(s * 100))
