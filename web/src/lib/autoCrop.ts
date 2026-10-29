import type { QueryClient } from '@tanstack/react-query'
import { api } from '@/api/client'
import type { AutoCropBody, AutoCropKind, AutoCropResult, CropOp, EditStack } from '@/api/types'
import { useEdit } from '@/stores/edit'
import { useToasts } from '@/stores/toasts'
import { aspectRatio, getCrop, setCrop, stacksEqual } from './edit'
import { changeStack } from './editActions'
import { errorText } from '@/lib/errors'

/** |angle| below this (degrees) counts as "no rotation" when deciding whether a level photo needs a change. */
export const LEVEL_EPS = 0.05

/** Request for `POST /edits/{id}/auto-crop`: the chosen aspect becomes a pixel ratio, the current crop rides along. */
export function autoCropBody(kind: AutoCropKind, crop: CropOp | undefined, imgAspect: number): AutoCropBody {
  const body: AutoCropBody = { kind }
  const ratio = aspectRatio(crop?.aspect ?? 'original', imgAspect)
  if (ratio) body.ratio = ratio
  if (crop) {
    body.rect = crop.rect
    body.angle = crop.angle
  }
  return body
}

export type AutoCropOutcome = 'applied' | 'level' | 'none' | 'unchanged'

/**
 * Fold a server suggestion into the stack. `none`: nothing confident was found; `level`: the photo is
 * level and has no rotation to remove (stack untouched); `unchanged`: the suggestion equals the current crop.
 */
export function applyAutoCrop(stack: EditStack, res: AutoCropResult): { stack: EditStack; outcome: AutoCropOutcome } {
  if (!res.crop) return { stack, outcome: 'none' }
  const cur = getCrop(stack)
  if (res.level && Math.abs(cur?.angle ?? 0) < LEVEL_EPS) return { stack, outcome: 'level' }
  const next = setCrop(stack, { rect: res.crop.rect, angle: res.crop.angle, aspect: cur?.aspect ?? 'original' })
  return stacksEqual(next, stack) ? { stack, outcome: 'unchanged' } : { stack: next, outcome: 'applied' }
}

/** Rounded for display: `-3` not `-3.00`. */
export const fmtDeg = (v: number) => String(Math.round(v * 10) / 10)

/**
 * Ask the server for a suggestion and put it in the edit stack as ONE undo step (non-destructive:
 * only the crop op changes). Returns the outcome; toasts explain the non-applied ones.
 */
export async function runAutoCrop(
  qc: QueryClient,
  kind: AutoCropKind,
  imgAspect: number,
  text: { label: string; done: (res: AutoCropResult) => string; level: string; none: string },
): Promise<AutoCropOutcome | 'error'> {
  const photoId = useEdit.getState().photoId
  if (photoId === null) return 'error'
  const toasts = useToasts.getState()
  try {
    const res = await api.autoCrop(photoId, autoCropBody(kind, getCrop(useEdit.getState().stack), imgAspect))
    if (useEdit.getState().photoId !== photoId) return 'error'
    const { stack, outcome } = applyAutoCrop(useEdit.getState().stack, res)
    if (outcome === 'applied') {
      changeStack(qc, stack, text.label)
      toasts.push('success', text.done(res), 3500)
    } else if (outcome === 'level') toasts.push('info', text.level, 3500)
    else if (outcome === 'none') toasts.push('info', text.none, 5000)
    return outcome
  } catch (err) {
    toasts.push('error', errorText(err), 5000)
    return 'error'
  }
}
