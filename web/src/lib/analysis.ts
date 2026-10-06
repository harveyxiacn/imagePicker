import type { QueryClient } from '@tanstack/react-query'
import { api, ApiError } from '@/api/client'
import type { AnalysisProfile, AnalysisStatus } from '@/api/types'
import { useAnalysisUi } from '@/stores/analysis'
import { useToasts } from '@/stores/toasts'
import { qk } from './cache'

/** Extracts the `models` list of a 409 models_missing response (contract C.2). */
export function missingModelsOf(err: unknown): string[] | null {
  if (!(err instanceof ApiError) || err.status !== 409 || err.code !== 'models_missing') return null
  const models = (err.body as { models?: unknown } | undefined)?.models
  return Array.isArray(models) ? models.filter((m): m is string => typeof m === 'string') : []
}

/**
 * Start (or resume) analysis. A 409 models_missing opens the consent dialog instead of failing;
 * the dialog calls `/api/models/ensure` and then retries through this same function.
 */
export async function startAnalysis(
  qc: QueryClient,
  sessionId: number,
  profile: AnalysisProfile,
  photoIds?: number[],
): Promise<boolean> {
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
    useToasts.getState().push('error', err instanceof Error ? err.message : String(err), 6000)
    return false
  }
}

export async function cancelAnalysis(qc: QueryClient, sessionId: number): Promise<void> {
  try {
    await api.cancelAnalysis(sessionId)
  } catch (err) {
    useToasts.getState().push('error', err instanceof Error ? err.message : String(err))
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
