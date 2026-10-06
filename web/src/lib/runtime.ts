import { api, ApiError } from '@/api/client'
import type { RuntimeInfo } from '@/api/types'
import i18n from '@/i18n'
import { useToasts } from '@/stores/toasts'

/** True for the `503 worker_unavailable` whose body says the AI runtime is not installed (`runtime: "missing"`). */
export function isRuntimeMissing(err: unknown): boolean {
  if (!(err instanceof ApiError) || err.status !== 503) return false
  return (err.body as { runtime?: unknown } | undefined)?.runtime === 'missing'
}

export interface RuntimeView {
  /** i18n key under `runtime.` */
  labelKey: 'missing' | 'outdated' | 'installing' | 'ready' | 'failed'
  tone: 'muted' | 'ai' | 'success' | 'danger'
  /** 0..100 while installing, otherwise null */
  percent: number | null
  /** i18n key under `runtime.step_` of the running step */
  stepKey: string | null
  detail: string
  canInstall: boolean
  canCancel: boolean
  canRemove: boolean
  /** installing again makes sense (missing / failed / outdated) vs. already ready */
  needsInstall: boolean
}

/** Pure view-model of `GET /api/runtime`: what the onboarding card and the settings section show. */
export function runtimeView(rt: RuntimeInfo | undefined): RuntimeView {
  const state = rt?.state ?? 'missing'
  const installing = state === 'installing'
  const labelKey: RuntimeView['labelKey'] = installing ? 'installing' : state === 'ready' ? 'ready' : state === 'failed' ? 'failed' : rt?.outdated ? 'outdated' : 'missing'
  return {
    labelKey,
    tone: installing ? 'ai' : state === 'ready' ? 'success' : state === 'failed' ? 'danger' : 'muted',
    percent: installing ? Math.max(0, Math.min(100, Math.round(rt?.percent ?? 0))) : null,
    stepKey: installing && rt?.step ? `step_${rt.step}` : null,
    detail: installing ? (rt?.detail ?? '') : '',
    canInstall: !!rt && rt.can_install && !installing,
    canCancel: installing,
    canRemove: !!rt && !installing && (state === 'ready' || state === 'failed' || rt.version !== null),
    needsInstall: state !== 'ready' && !installing,
  }
}

export interface InstallDeps {
  install: () => Promise<unknown>
  get: () => Promise<RuntimeInfo>
  sleep: (ms: number) => Promise<void>
}

const realDeps: InstallDeps = {
  install: () => api.installRuntime(),
  get: () => api.runtime(),
  sleep: (ms) => new Promise((r) => setTimeout(r, ms)),
}

/**
 * Installs the AI runtime when it is missing and resolves once it is ready; `onProgress` sees every poll.
 * Rejects with the install error (or when the install was cancelled). Resolves at once when already ready.
 */
export async function installRuntimeAndWait(onProgress?: (rt: RuntimeInfo) => void, deps: InstallDeps = realDeps, pollMs = 1000): Promise<RuntimeInfo> {
  let rt = await deps.get()
  if (rt.state === 'ready') return rt
  if (rt.state !== 'installing') {
    if (!rt.can_install) throw new Error(rt.reason ?? i18n.t('runtime.cannotInstall'))
    await deps.install()
  }
  for (;;) {
    await deps.sleep(pollMs)
    rt = await deps.get()
    onProgress?.(rt)
    if (rt.state === 'ready') return rt
    if (rt.state === 'failed') throw new Error(rt.error ?? i18n.t('runtime.failed'))
    if (rt.state === 'missing') throw new Error(i18n.t('runtime.cancelled'))
  }
}

/**
 * When `err` is "runtime missing", show a toast offering to install the AI components and return true.
 * Callers show their own error otherwise.
 */
export function offerRuntimeInstall(err: unknown, after?: () => void): boolean {
  if (!isRuntimeMissing(err)) return false
  useToasts.getState().push('error', i18n.t('runtime.missingToast'), 10_000, [
    {
      label: i18n.t('runtime.install'),
      onClick: () => {
        void api
          .installRuntime()
          .then(() => {
            useToasts.getState().push('info', i18n.t('runtime.installStarted'), 4000)
            if (after) void installRuntimeAndWait().then(after, () => undefined)
          })
          .catch((e) => useToasts.getState().push('error', e instanceof Error ? e.message : String(e), 6000))
      },
    },
  ])
  return true
}
