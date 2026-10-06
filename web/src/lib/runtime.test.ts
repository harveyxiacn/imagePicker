import { describe, expect, it } from 'vitest'
import { ApiError } from '@/api/client'
import type { RuntimeInfo } from '@/api/types'
import { installRuntimeAndWait, isRuntimeMissing, runtimeView } from './runtime'

const base: RuntimeInfo = {
  state: 'missing',
  version: null,
  bundled_version: '0.1.0',
  outdated: false,
  extras: [],
  python: null,
  venv_bytes: null,
  error: null,
  can_install: true,
  reason: null,
  step: null,
  percent: 0,
  detail: '',
  task_id: null,
}

describe('runtimeView', () => {
  it('missing: offers the install, nothing to cancel or remove', () => {
    const v = runtimeView(base)
    expect(v).toMatchObject({ labelKey: 'missing', tone: 'muted', percent: null, canInstall: true, canCancel: false, canRemove: false, needsInstall: true })
  })

  it('outdated runtime asks for an update', () => {
    const v = runtimeView({ ...base, outdated: true, version: '0.0.9' })
    expect(v.labelKey).toBe('outdated')
    expect(v.canRemove).toBe(true)
    expect(v.needsInstall).toBe(true)
  })

  it('installing: progress, step, cancel only', () => {
    const v = runtimeView({ ...base, state: 'installing', percent: 41.6, step: 'sync', detail: 'Downloading numpy', can_install: false })
    expect(v).toMatchObject({ labelKey: 'installing', tone: 'ai', percent: 42, stepKey: 'step_sync', detail: 'Downloading numpy', canInstall: false, canCancel: true, canRemove: false })
  })

  it('percent is clamped', () => {
    expect(runtimeView({ ...base, state: 'installing', percent: 250 }).percent).toBe(100)
  })

  it('ready: can remove and reinstall, nothing is needed', () => {
    const v = runtimeView({ ...base, state: 'ready', version: '0.1.0', extras: ['cuda'] })
    expect(v).toMatchObject({ labelKey: 'ready', tone: 'success', needsInstall: false, canRemove: true, canInstall: true })
  })

  it('failed keeps the error for the row and allows a retry', () => {
    const v = runtimeView({ ...base, state: 'failed', error: 'boom' })
    expect(v).toMatchObject({ labelKey: 'failed', tone: 'danger', needsInstall: true, canInstall: true })
  })

  it('network off: cannot install', () => {
    expect(runtimeView({ ...base, can_install: false, reason: 'network access is disabled' }).canInstall).toBe(false)
  })

  it('no data yet behaves like missing without actions', () => {
    const v = runtimeView(undefined)
    expect(v.labelKey).toBe('missing')
    expect(v.canInstall).toBe(false)
  })
})

describe('isRuntimeMissing', () => {
  it('recognises the 503 hint only', () => {
    expect(isRuntimeMissing(new ApiError(503, 'worker_unavailable', 'x', { error: {}, runtime: 'missing' }))).toBe(true)
    expect(isRuntimeMissing(new ApiError(503, 'worker_unavailable', 'x', { error: {} }))).toBe(false)
    expect(isRuntimeMissing(new ApiError(409, 'models_missing', 'x', { runtime: 'missing' }))).toBe(false)
    expect(isRuntimeMissing(new Error('x'))).toBe(false)
  })
})

describe('installRuntimeAndWait', () => {
  const run = (states: Partial<RuntimeInfo>[]) => {
    const calls = { install: 0, seen: [] as number[] }
    let i = 0
    const deps = {
      install: async () => {
        calls.install++
      },
      get: async () => ({ ...base, ...states[Math.min(i++, states.length - 1)] }),
      sleep: async () => undefined,
    }
    return { calls, deps }
  }

  it('returns at once when ready', async () => {
    const { calls, deps } = run([{ state: 'ready' }])
    await installRuntimeAndWait(undefined, deps)
    expect(calls.install).toBe(0)
  })

  it('installs, reports progress and resolves when ready', async () => {
    const { calls, deps } = run([{ state: 'missing' }, { state: 'installing', percent: 10 }, { state: 'installing', percent: 70 }, { state: 'ready' }])
    const seen: number[] = []
    const rt = await installRuntimeAndWait((r) => seen.push(r.percent), deps, 0)
    expect(rt.state).toBe('ready')
    expect(calls.install).toBe(1)
    expect(seen).toEqual([10, 70, 0])
  })

  it('joins an install that is already running', async () => {
    const { calls, deps } = run([{ state: 'installing' }, { state: 'ready' }])
    await installRuntimeAndWait(undefined, deps, 0)
    expect(calls.install).toBe(0)
  })

  it('rejects with the install error', async () => {
    const { deps } = run([{ state: 'missing' }, { state: 'failed', error: 'no disk space' }])
    await expect(installRuntimeAndWait(undefined, deps, 0)).rejects.toThrow('no disk space')
  })

  it('rejects when nothing can be installed', async () => {
    const { calls, deps } = run([{ state: 'missing', can_install: false, reason: 'offline' }])
    await expect(installRuntimeAndWait(undefined, deps, 0)).rejects.toThrow('offline')
    expect(calls.install).toBe(0)
  })

  it('treats a vanished install as cancelled', async () => {
    const { deps } = run([{ state: 'missing' }, { state: 'installing' }, { state: 'missing' }])
    await expect(installRuntimeAndWait(undefined, deps, 0)).rejects.toThrow()
  })
})
