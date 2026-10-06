/**
 * Pure reducers for the portrait ops (docs/api-contract-m4.md A): at most ONE `beauty`, ONE `warp face` and
 * ONE `warp body` op per `person_id` (`null` = "everyone"). Every function returns a new stack; the result is
 * normalised so a person whose sliders all return to neutral has no ops left.
 */
import {
  BEAUTY_KEYS,
  WARP_BODY_KEYS,
  WARP_FACE_KEYS,
  type BeautyKey,
  type BeautyOp,
  type BeautyProfile,
  type EditStack,
  type Level,
  type Op,
  type PortraitOp,
  type WarpBodyKey,
  type WarpBodyOp,
  type WarpFaceKey,
  type WarpFaceOp,
} from '@/api/types'
import { clamp, isBeauty, isNeutralPortrait, isPortrait, isWarpBody, isWarpFace, normalizeStack, orderOps } from './edit'

/** `null` = every face; a number = that person. */
export type PersonKey = number | null

export const DEFAULT_LEVEL: Level = 'standard'

const pid = (o: PortraitOp): PersonKey => o.person_id ?? null
const forPerson = (o: Op, p: PersonKey): o is PortraitOp => isPortrait(o) && pid(o) === p

const rebuild = (s: EditStack, ops: Op[]): EditStack => normalizeStack({ version: s.version ?? 1, ops: orderOps(ops) })

/** Optional `person_id` is omitted from the wire form when it is null (matches serde `skip_serializing_if`). */
function withPerson<T extends { person_id?: number | null }>(op: T, p: PersonKey): T {
  const out = { ...op }
  if (p === null) delete out.person_id
  else out.person_id = p
  return out
}

// ---------------------------------------------------------------- defaults

export const newBeauty = (p: PersonKey, level: Level = DEFAULT_LEVEL): BeautyOp =>
  withPerson<BeautyOp>({ type: 'beauty', level, smooth: 0, whiten: 0, blemish: false, eye_brighten: 0, teeth_whiten: 0, dark_circles: 0 }, p)
export const newWarpFace = (p: PersonKey, level: Level = DEFAULT_LEVEL): WarpFaceOp =>
  withPerson<WarpFaceOp>({ type: 'warp', kind: 'face', level, slim: 0, chin: 0, eyes: 0, nose: 0 }, p)
export const newWarpBody = (p: PersonKey, level: Level = DEFAULT_LEVEL): WarpBodyOp =>
  withPerson<WarpBodyOp>({ type: 'warp', kind: 'body', level, arms: 0, legs: 0, waist: 0, lengthen_legs: 0, protect_background: true }, p)

// ---------------------------------------------------------------- ranges

export interface PortraitSlider<K extends string> {
  key: K
  min: number
  max: number
}
const range = <K extends string>(keys: readonly K[], min: number, max: number): PortraitSlider<K>[] => keys.map((key) => ({ key, min, max }))
export const BEAUTY_SLIDERS = range(BEAUTY_KEYS, 0, 100)
export const FACE_SLIDERS = range(WARP_FACE_KEYS, -100, 100)
export const BODY_SLIDERS = range(WARP_BODY_KEYS, 0, 100)

// ---------------------------------------------------------------- getters

export const getBeauty = (s: EditStack, p: PersonKey): BeautyOp | undefined => s.ops.find((o): o is BeautyOp => forPerson(o, p) && isBeauty(o))
export const getWarpFace = (s: EditStack, p: PersonKey): WarpFaceOp | undefined => s.ops.find((o): o is WarpFaceOp => forPerson(o, p) && isWarpFace(o))
export const getWarpBody = (s: EditStack, p: PersonKey): WarpBodyOp | undefined => s.ops.find((o): o is WarpBodyOp => forPerson(o, p) && isWarpBody(o))

/** Level shown for a person: that of the first of their ops, else `fallback`. */
export function getLevel(s: EditStack, p: PersonKey, fallback: Level = DEFAULT_LEVEL): Level {
  return getBeauty(s, p)?.level ?? getWarpFace(s, p)?.level ?? getWarpBody(s, p)?.level ?? fallback
}

/** Person keys that own at least one portrait op. */
export function portraitPeople(s: EditStack): PersonKey[] {
  const seen: PersonKey[] = []
  for (const o of s.ops) if (isPortrait(o) && !seen.includes(pid(o))) seen.push(pid(o))
  return seen
}

export const hasPortrait = (s: EditStack, p: PersonKey): boolean => s.ops.some((o) => forPerson(o, p) && !isNeutralPortrait(o))

// ---------------------------------------------------------------- generic upsert (one op per kind per person)

type Kind = 'beauty' | 'face' | 'body'
const matches = (o: Op, kind: Kind, p: PersonKey): boolean =>
  forPerson(o, p) && (kind === 'beauty' ? isBeauty(o) : kind === 'face' ? isWarpFace(o) : isWarpBody(o))

/**
 * Update the (single) op of `kind` for person `p`, creating it when absent. Any further duplicates (hand-edited
 * stacks) are dropped so the one-per-kind-per-person invariant holds after every edit.
 */
function upsert<T extends PortraitOp>(s: EditStack, kind: Kind, p: PersonKey, make: () => T, fn: (cur: T) => T): EditStack {
  const cur = s.ops.find((o) => matches(o, kind, p)) as T | undefined
  const next = fn(cur ?? make())
  const rest = s.ops.filter((o) => !matches(o, kind, p))
  return rebuild(s, [...rest, next])
}

export function setBeautyField(s: EditStack, p: PersonKey, key: BeautyKey, value: number, level: Level = DEFAULT_LEVEL): EditStack {
  return upsert(s, 'beauty', p, () => newBeauty(p, level), (o) => ({ ...o, [key]: clamp(value, 0, 100) }))
}
export function setBlemish(s: EditStack, p: PersonKey, on: boolean, level: Level = DEFAULT_LEVEL): EditStack {
  return upsert(s, 'beauty', p, () => newBeauty(p, level), (o) => ({ ...o, blemish: on }))
}
export function setFaceField(s: EditStack, p: PersonKey, key: WarpFaceKey, value: number, level: Level = DEFAULT_LEVEL): EditStack {
  return upsert(s, 'face', p, () => newWarpFace(p, level), (o) => ({ ...o, [key]: clamp(value, -100, 100) }))
}
export function setBodyField(s: EditStack, p: PersonKey, key: WarpBodyKey, value: number, level: Level = DEFAULT_LEVEL): EditStack {
  return upsert(s, 'body', p, () => newWarpBody(p, level), (o) => ({ ...o, [key]: clamp(value, 0, 100) }))
}
export function setProtectBackground(s: EditStack, p: PersonKey, on: boolean, level: Level = DEFAULT_LEVEL): EditStack {
  return upsert(s, 'body', p, () => newWarpBody(p, level), (o) => ({ ...o, protect_background: on }))
}

/** Change the level of every existing op of the person (the level scales + caps all amounts). */
export function setLevel(s: EditStack, p: PersonKey, level: Level): EditStack {
  return rebuild(
    s,
    s.ops.map((o) => (forPerson(o, p) ? { ...o, level } : o)),
  )
}

/** Remove every portrait op of one person (section reset). */
export const removePerson = (s: EditStack, p: PersonKey): EditStack =>
  rebuild(
    s,
    s.ops.filter((o) => !forPerson(o, p)),
  )

/** Remove one kind of ops (beauty / face / body) of one person. */
export const removeKind = (s: EditStack, kind: Kind, p: PersonKey): EditStack =>
  rebuild(
    s,
    s.ops.filter((o) => !matches(o, kind, p)),
  )

// ---------------------------------------------------------------- naturalness

/** Per-slider "this is a lot" thresholds used with the Refined level (UI hint only). */
const HIGH = 70

/** True when the Refined level is combined with strong values: the result is likely to look retouched. */
export function isUnnatural(s: EditStack, p: PersonKey): boolean {
  const b = getBeauty(s, p)
  const f = getWarpFace(s, p)
  const w = getWarpBody(s, p)
  const refined = [b, f, w].some((o) => o?.level === 'refined')
  if (!refined) return false
  const high = (v: number | undefined) => Math.abs(v ?? 0) >= HIGH
  return (
    (b?.level === 'refined' && (high(b.smooth) || high(b.whiten) || high(b.eye_brighten) || high(b.teeth_whiten))) ||
    (f?.level === 'refined' && (high(f.slim) || high(f.chin) || high(f.eyes) || high(f.nose))) ||
    (w?.level === 'refined' && (high(w.arms) || high(w.legs) || high(w.waist) || high(w.lengthen_legs)))
  )
}

// ---------------------------------------------------------------- profiles (person look <-> ops)

const strip = <T extends { type: string; person_id?: number | null }>(o: T): Omit<T, 'type' | 'person_id'> => {
  const { type: _t, person_id: _p, ...rest } = o
  void _t
  void _p
  return rest
}

/** The saved-look form of person `p`'s ops; `null` when they have nothing non-neutral. */
export function profileFromStack(s: EditStack, p: PersonKey): BeautyProfile | null {
  const b = getBeauty(s, p)
  const f = getWarpFace(s, p)
  const w = getWarpBody(s, p)
  const out: BeautyProfile = {}
  if (b && !isNeutralPortrait(b)) out.beauty = strip(b)
  if (f && !isNeutralPortrait(f)) {
    const { kind: _k, ...rest } = strip(f)
    void _k
    out.face = rest
  }
  if (w && !isNeutralPortrait(w)) {
    const { kind: _k, ...rest } = strip(w)
    void _k
    out.body = rest
  }
  return out.beauty || out.face || out.body ? out : null
}

/** Replace person `p`'s ops by the profile (kinds the profile lacks are cleared); `p` is filled in as `person_id`. */
export function applyProfile(s: EditStack, profile: BeautyProfile, p: PersonKey): EditStack {
  const rest = s.ops.filter((o) => !forPerson(o, p))
  const add: Op[] = []
  if (profile.beauty) add.push(withPerson({ ...newBeauty(p), ...profile.beauty, type: 'beauty' as const }, p))
  if (profile.face) add.push(withPerson({ ...newWarpFace(p), ...profile.face, type: 'warp' as const, kind: 'face' as const }, p))
  if (profile.body) add.push(withPerson({ ...newWarpBody(p), ...profile.body, type: 'warp' as const, kind: 'body' as const }, p))
  return rebuild(s, [...rest, ...add])
}

/** Which kinds a profile carries (UI summary chips). */
export const profileKinds = (pr: BeautyProfile): Kind[] => (['beauty', 'face', 'body'] as const).filter((k) => !!(k === 'beauty' ? pr.beauty : k === 'face' ? pr.face : pr.body))
