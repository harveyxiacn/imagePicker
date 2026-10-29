import { QueryClient } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { api, ApiError } from '@/api/client'
import type { AnalysisStatus, Settings } from '@/api/types'
import i18n from '@/i18n'
import { useAnalysisUi } from '@/stores/analysis'
import { useToasts } from '@/stores/toasts'
import { analysisActions, analysisActive, analysisFraction, continueAnalysis, isWorkerUnavailable, pauseAnalysis, startAnalysis } from './analysis'
import { qk } from './cache'

const status = (over: Partial<AnalysisStatus> = {}): AnalysisStatus => ({
  state: 'idle',
  profile: 'standard',
  done: 0,
  total: 0,
  stage: null,
  error: null,
  ...over,
})

describe('analysisActions', () => {
  it('a running analysis can be paused or cancelled, not continued', () => {
    expect(analysisActions(status({ state: 'running' }))).toEqual({ canPause: true, canContinue: false, canCancel: true, offerLite: false })
  })

  it('a pause that was requested is not offered again', () => {
    expect(analysisActions(status({ state: 'running' }), true).canPause).toBe(false)
  })

  it('a paused analysis can be continued or cancelled', () => {
    expect(analysisActions(status({ state: 'paused' }))).toEqual({ canPause: false, canContinue: true, canCancel: true, offerLite: false })
  })

  it('offers the quick analysis after the AI side failed, but not for other failures or for lite itself', () => {
    expect(analysisActions(status({ state: 'failed', error: 'x', error_code: 'worker_unavailable' })).offerLite).toBe(true)
    expect(analysisActions(status({ state: 'failed', error: 'x', error_code: 'models_missing' })).offerLite).toBe(true)
    expect(analysisActions(status({ state: 'failed', error: 'x' })).offerLite).toBe(false)
    expect(analysisActions(status({ state: 'failed', error: 'x', error_code: 'worker_unavailable', profile: 'lite' })).offerLite).toBe(false)
    expect(analysisActions(status({ state: 'done' }))).toEqual({ canPause: false, canContinue: false, canCancel: false, offerLite: false })
    expect(analysisActions(undefined).canCancel).toBe(false)
  })

  it('a paused run still counts as active; progress keeps its fraction', () => {
    expect(analysisActive(status({ state: 'paused' }))).toBe(true)
    expect(analysisActive(status({ state: 'running' }))).toBe(true)
    expect(analysisActive(status({ state: 'done' }))).toBe(false)
    expect(analysisFraction(status({ state: 'paused', done: 50, total: 100, stage: null }))).toBeCloseTo(0.45)
  })
})

describe('isWorkerUnavailable', () => {
  it('matches the 503 worker_unavailable only', () => {
    expect(isWorkerUnavailable(new ApiError(503, 'worker_unavailable', 'x'))).toBe(true)
    expect(isWorkerUnavailable(new ApiError(409, 'models_missing', 'x'))).toBe(false)
    expect(isWorkerUnavailable(new Error('x'))).toBe(false)
  })
})

describe('starting, pausing and continuing', () => {
  let qc: QueryClient
  beforeEach(() => {
    qc = new QueryClient()
    qc.setQueryData<Settings>(qk.settings, { faces: { enabled: true, consented: true } } as Settings)
    useToasts.setState({ toasts: [], tasks: {} })
    useAnalysisUi.setState({ consent: null, pausing: {} })
  })
  afterEach(() => vi.restoreAllMocks())

  it('missing models open the consent dialog (which offers the quick analysis)', async () => {
    vi.spyOn(api, 'runAnalysis').mockRejectedValue(new ApiError(409, 'models_missing', 'm', { models: ['siglip2-base'] }))
    expect(await startAnalysis(qc, 7, 'standard', [1, 2])).toBe(false)
    expect(useAnalysisUi.getState().consent).toMatchObject({ sessionId: 7, profile: 'standard', photoIds: [1, 2], models: ['siglip2-base'] })
  })

  it('a missing runtime offers installing it and the quick analysis for the same photos', async () => {
    const run = vi.spyOn(api, 'runAnalysis').mockRejectedValueOnce(new ApiError(503, 'worker_unavailable', 'x', { error: {}, runtime: 'missing' }))
    expect(await startAnalysis(qc, 7, 'standard', [3])).toBe(false)
    const toast = useToasts.getState().toasts[0]
    expect(toast.actions).toHaveLength(2)
    run.mockResolvedValue({ task_id: 'analysis-1' })
    toast.actions![1].onClick()
    await vi.waitFor(() => expect(run).toHaveBeenLastCalledWith({ session_id: 7, profile: 'lite', photo_ids: [3] }))
  })

  it('an unreachable worker offers the quick analysis; lite itself just reports the error', async () => {
    const run = vi.spyOn(api, 'runAnalysis').mockRejectedValue(new ApiError(503, 'worker_unavailable', 'worker down'))
    await startAnalysis(qc, 7, 'fast')
    expect(useToasts.getState().toasts[0].actions).toHaveLength(1)
    useToasts.setState({ toasts: [] })
    await startAnalysis(qc, 7, 'lite')
    expect(run).toHaveBeenCalledTimes(2)
    expect(useToasts.getState().toasts[0]).toMatchObject({ kind: 'error', text: i18n.t('errors.workerUnavailable') })
    expect(useToasts.getState().toasts[0].actions).toBeUndefined()
  })

  it('a started run shows as running at once', async () => {
    vi.spyOn(api, 'runAnalysis').mockResolvedValue({ task_id: 'analysis-1' })
    expect(await startAnalysis(qc, 7, 'lite')).toBe(true)
    expect(qc.getQueryData<AnalysisStatus>(qk.analysisStatus(7))).toMatchObject({ state: 'running', profile: 'lite' })
  })

  it('pausing marks the session as pausing; a refused pause clears it again', async () => {
    const pause = vi.spyOn(api, 'pauseAnalysis').mockResolvedValue(undefined)
    await pauseAnalysis(7)
    expect(pause).toHaveBeenCalledWith(7)
    expect(useAnalysisUi.getState().pausing).toEqual({ 7: true })
    pause.mockRejectedValue(new ApiError(409, 'conflict', 'no analysis is running'))
    await pauseAnalysis(8)
    expect(useAnalysisUi.getState().pausing).toEqual({ 7: true })
    expect(useToasts.getState().toasts[0].kind).toBe('error')
  })

  it('continuing flips the cached status back to running', async () => {
    qc.setQueryData<AnalysisStatus>(qk.analysisStatus(7), status({ state: 'paused', done: 32, total: 70 }))
    const cont = vi.spyOn(api, 'continueAnalysis').mockResolvedValue({ task_id: 'analysis-1' })
    await continueAnalysis(qc, 7)
    expect(cont).toHaveBeenCalledWith(7)
    expect(qc.getQueryData<AnalysisStatus>(qk.analysisStatus(7))).toMatchObject({ state: 'running', stage: 'analyzing', done: 32, total: 70 })
    cont.mockRejectedValue(new ApiError(409, 'conflict', 'no analysis is paused'))
    qc.setQueryData<AnalysisStatus>(qk.analysisStatus(7), status({ state: 'paused' }))
    await continueAnalysis(qc, 7)
    expect(qc.getQueryData<AnalysisStatus>(qk.analysisStatus(7))?.state).toBe('paused')
  })
})
