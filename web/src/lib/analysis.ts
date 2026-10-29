import type { QueryClient } from '@tanstack/react-query'
import { api, ApiError } from '@/api/client'
import type { AnalysisProfile, AnalysisStatus } from '@/api/types'
import i18n from '@/i18n'
import { useAnalysisUi } from '@/stores/analysis'
import { useFaceConsent } from '@/stores/faceConsent'
import { useToasts } from '@/stores/toasts'
import { qk } from './cache'
import { faceConsentPending } from './faceConsent'
import { offerRuntimeInstall } from './runtime'
import { errorText } from '@/lib/errors'

/** Extracts the `models` list of a 409 models_missing response (contract C.2). */
export function missingModelsOf(err: unknown): string[] | null {
  if (!(err instanceof ApiError) || err.status !== 409 || err.code !== 'models_missing') return null
  const models = (err.body as { models?: unknown } | undefined)?.models
  return Array.isArray(models) ? models.filter((m): m is string => typeof m === 'string') : []
}

/** True for a 503 worker_unavailable: the AI worker cannot be reached or started. */
export function isWorkerUnavailable(err: unknown): boolean {
  return err instanceof ApiError && err.status === 503 && err.code === 'worker_unavailable'
}

/** What may be offered for a run that is going on or stopped, derived from its status. */
export interface AnalysisActions {
  canPause: boolean
  canContinue: boolean
  canCancel: boolean
  /** the AI side failed: the photos left can be finished with the on-device `lite` profile */
  offerLite: boolean
}

/** `pausing`: a pause was requested and the batch in flight has not finished yet. */
export function analysisActions(s: AnalysisStatus | undefined, pausing = false): AnalysisActions {
  const state = s?.state
  return {
    canPause: state === 'running' && !pausing,
    canContinue: state === 'paused',
    canCancel: state === 'running' || state === 'paused',
    offerLite: state === 'failed' && !!s?.error_code && s.profile !== 'lite',
  }
}

/** A run is on its way or waiting to be continued: no new one can start. */
export const analysisActive = (s: Pick<AnalysisStatus, 'state'> | undefined): boolean => s?.state === 'running' || s?.state === 'paused'

/**
 * Start (or resume) analysis. A 409 models_missing opens the consent dialog instead of failing;
 * the dialog calls `/api/models/ensure` and then retries through this same function. When the AI
 * cannot be used at all (components not installed, worker unreachable) the error comes with a
 * "use quick analysis" choice that runs the on-device `lite` profile for the same photos.
 * Before the first analysis the face-recognition question is asked (docs/02 §8); its dialog
 * also retries through here once answered.
 */
export async function startAnalysis(
  qc: QueryClient,
  sessionId: number,
  profile: AnalysisProfile,
  photoIds?: number[],
): Promise<boolean> {
  if (await faceConsentPending(qc)) {
    useFaceConsent.getState().ask({ sessionId, profile, photoIds })
    return false
  }
  try {
    await api.runAnalysis({ session_id: sessionId, profile, ...(photoIds ? { photo_ids: photoIds } : {}) })
    qc.setQueryData<AnalysisStatus>(qk.analysisStatus(sessionId), {
      state: 'running',
      profile,
      done: 0,
      total: 0,
      stage: 'analyzing',
      error: null,
    })
    return true
  } catch (err) {
    const missing = missingModelsOf(err)
    if (missing) {
      useAnalysisUi.getState().setConsent({ sessionId, profile, photoIds, models: missing })
      return false
    }
    const lite =
      profile === 'lite'
        ? undefined
        : { label: i18n.t('analysis.useLite'), onClick: () => void startAnalysis(qc, sessionId, 'lite', photoIds) }
    if (offerRuntimeInstall(err, undefined, lite)) return false
    if (lite && isWorkerUnavailable(err)) {
      useToasts.getState().push('error', i18n.t('analysis.liteOffer'), 10_000, [lite])
      return false
    }
    useToasts.getState().push('error', errorText(err), 6000)
    return false
  }
}

/** Asks the running analysis to stop after its current batch; the status turns `paused` by event. */
export async function pauseAnalysis(sessionId: number): Promise<void> {
  const ui = useAnalysisUi.getState()
  ui.setPausing(sessionId, true)
  try {
    await api.pauseAnalysis(sessionId)
  } catch (err) {
    ui.setPausing(sessionId, false)
    useToasts.getState().push('error', errorText(err))
  }
}

/** Takes a paused analysis up again with the photos that are left. */
export async function continueAnalysis(qc: QueryClient, sessionId: number): Promise<void> {
  try {
    await api.continueAnalysis(sessionId)
  } catch (err) {
    useToasts.getState().push('error', errorText(err))
    return
  }
  qc.setQueryData<AnalysisStatus>(qk.analysisStatus(sessionId), (old) =>
    old ? { ...old, state: 'running', stage: 'analyzing', error: null, error_code: null } : old,
  )
}

export async function cancelAnalysis(qc: QueryClient, sessionId: number): Promise<void> {
  try {
    await api.cancelAnalysis(sessionId)
  } catch (err) {
    useToasts.getState().push('error', errorText(err))
    return
  }
  qc.setQueryData<AnalysisStatus>(qk.analysisStatus(sessionId), (old) => (old ? { ...old, state: 'idle', stage: null } : old))
}

/** Progress as 0..1 across the staged pipeline: analyzing dominates, later stages fill the tail. */
export function analysisFraction(s: Pick<AnalysisStatus, 'state' | 'stage' | 'done' | 'total'>): number {
  if (s.state === 'done') return 1
  const base = s.total > 0 ? s.done / s.total : 0
  switch (s.stage) {
    case 'grouping':
      return 0.9
    case 'scoring':
      return 0.94
    case 'clustering':
      return 0.97
    default:
      return Math.min(0.9, base * 0.9)
  }
}
