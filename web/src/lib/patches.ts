/**
 * Pure reducers for the M5 `patch` layers of an edit stack (docs/api-contract-m5.md A).
 * Patches render first (before crop), in stack order; `enabled:false` keeps a layer in the stack without rendering it.
 * Every function returns a new stack and never mutates its input.
 */
import type { EditStack, Op, PatchKind, PatchOp } from '@/api/types'
import { isPatch, normalizeStack, orderOps } from './edit'

export { isPatch }

const finish = (s: EditStack, ops: Op[]): EditStack => normalizeStack({ version: s.version ?? 1, ops: orderOps(ops) })

export interface PatchEntry {
  op: PatchOp
  /** index within `stack.ops` */
  opIndex: number
  /** index among patch ops (stable identity for the UI) */
  index: number
}

export function patchEntries(s: EditStack): PatchEntry[] {
  const out: PatchEntry[] = []
  s.ops.forEach((op, opIndex) => {
    if (isPatch(op)) out.push({ op, opIndex, index: out.length })
  })
  return out
}

export const patchesOfKind = (s: EditStack, kind: PatchKind): PatchOp[] => patchEntries(s).filter((e) => e.op.kind === kind).map((e) => e.op)
export const isPatchEnabled = (o: PatchOp): boolean => o.enabled !== false
export const hasActivePatch = (s: EditStack, kind?: PatchKind): boolean => patchEntries(s).some((e) => isPatchEnabled(e.op) && (!kind || e.op.kind === kind))

/** Apply `fn` to the n-th patch op. */
export function updatePatch(s: EditStack, index: number, fn: (p: PatchOp) => PatchOp): EditStack {
  const e = patchEntries(s)[index]
  if (!e) return s
  const ops = [...s.ops]
  ops[e.opIndex] = fn(e.op)
  return finish(s, ops)
}

export const setPatchEnabled = (s: EditStack, index: number, enabled: boolean): EditStack => updatePatch(s, index, (p) => ({ ...p, enabled }))

export function removePatch(s: EditStack, index: number): EditStack {
  const e = patchEntries(s)[index]
  if (!e) return s
  return finish(
    s,
    s.ops.filter((_, i) => i !== e.opIndex),
  )
}

/** Append a patch layer on top of the existing ones. */
export const addPatch = (s: EditStack, patch: PatchOp): EditStack => finish(s, [...s.ops, patch])

/**
 * Replace the layers of `kind` selected by `match` (default: every layer of that kind) with `next`.
 * The first replaced layer keeps its position; when none matched the new layers go on top.
 */
export function replaceByKind(s: EditStack, kind: PatchKind, next: PatchOp[], match: (p: PatchOp) => boolean = () => true): EditStack {
  const first = s.ops.findIndex((o) => isPatch(o) && o.kind === kind && match(o))
  const kept = s.ops.filter((o) => !(isPatch(o) && o.kind === kind && match(o)))
  if (first < 0) return finish(s, [...kept, ...next])
  // position of the first replaced op among the kept ops
  const at = s.ops.slice(0, first).filter((o) => !(isPatch(o) && o.kind === kind && match(o))).length
  return finish(s, [...kept.slice(0, at), ...next, ...kept.slice(at)])
}

/** Best-take layer that replaced `personId`'s face (one per person). */
export const bestTakeOf = (s: EditStack, personId: number): PatchOp | undefined => patchesOfKind(s, 'best_take').find((p) => p.person_id === personId)

export const bestTakePersonIds = (s: EditStack): number[] =>
  patchesOfKind(s, 'best_take').flatMap((p) => (p.person_id !== undefined && isPatchEnabled(p) ? [p.person_id] : []))

/** "还原此人": drop the person's best-take layer. */
export function removeBestTake(s: EditStack, personId: number): EditStack {
  return finish(
    s,
    s.ops.filter((o) => !(isPatch(o) && o.kind === 'best_take' && o.person_id === personId)),
  )
}

export function updateBestTake(s: EditStack, personId: number, fn: (p: PatchOp) => PatchOp): EditStack {
  const idx = patchEntries(s).find((e) => e.op.kind === 'best_take' && e.op.person_id === personId)?.index
  return idx === undefined ? s : updatePatch(s, idx, fn)
}

/**
 * Take the patch layers from `server` and everything else from `live`: used when a generative task finished while
 * the user kept editing other controls (the server wrote the patches, the client holds newer sliders).
 */
export function mergePatches(live: EditStack, server: EditStack): EditStack {
  const rest = live.ops.filter((o) => !isPatch(o))
  return finish(live, [...server.ops.filter(isPatch), ...rest])
}

/** i18n key of a layer's display name. */
export const patchLabelKey = (kind: PatchKind): string => `repair.kind_${kind}`
