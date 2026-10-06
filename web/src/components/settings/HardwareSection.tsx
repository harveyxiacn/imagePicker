import { useQueryClient } from '@tanstack/react-query'
import { AlertTriangle, Check, Download, Loader2, Trash2 } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { api } from '@/api/client'
import { useAssistantStatus, useHardware, useModels } from '@/api/queries'
import type { ModelInfo, ModelSource } from '@/api/types'
import { formatBytes } from '@/lib/format'
import { offerRuntimeInstall } from '@/lib/runtime'
import { useToasts } from '@/stores/toasts'
import { RuntimeCard } from './RuntimeCard'
import { Card, ConfirmDialog, Row, Select } from './controls'
import { useSetting } from './useSetting'

const mb = (n: number) => (n >= 1024 ? `${(n / 1024).toFixed(1)} GB` : n >= 10 ? `${Math.round(n)} MB` : `${n} MB`)
const SOURCES: ModelSource[] = ['auto', 'hf', 'hf-mirror', 'modelscope']
const ENGINES = ['auto', 'rules', 'llm'] as const

/** 硬件与模型: worker status / tier, assistant engine, model source, models table (download / delete). */
export function HardwareSection() {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const { s, set } = useSetting()
  const hw = useHardware()
  const models = useModels()
  const status = useAssistantStatus()
  const [tasks, setTasks] = useState<Record<string, string>>({}) // model id -> task id
  const taskState = useToasts((x) => x.tasks)
  const [askDownload, setAskDownload] = useState<ModelInfo | null>(null)
  const [askDelete, setAskDelete] = useState<ModelInfo | null>(null)
  const offline = s ? !s.privacy.allow_network : false

  // finished downloads: refresh the table once per finished task
  const finished = Object.values(tasks).filter((tid) => taskState[tid]?.state === 'done' || taskState[tid]?.state === 'failed').length
  useEffect(() => {
    if (finished) void qc.invalidateQueries({ queryKey: ['models'] })
  }, [finished, qc])

  const download = async (m: ModelInfo) => {
    try {
      const { task_id } = await api.ensureModels([m.id])
      setTasks((cur) => ({ ...cur, [m.id]: task_id }))
      useToasts.getState().updateTask({ type: 'task.progress', task_id, kind: 'model_download', done: 0, total: Math.round(m.size_mb * 1e6), state: 'running' })
    } catch (e) {
      if (offerRuntimeInstall(e)) return
      useToasts.getState().push('error', e instanceof Error ? e.message : String(e), 6000)
    }
  }
  const remove = async (m: ModelInfo) => {
    try {
      await api.deleteModel(m.id)
      void qc.invalidateQueries({ queryKey: ['models'] })
      useToasts.getState().push('success', t('settings.models.deleted', { id: m.id }), 2500)
    } catch (e) {
      useToasts.getState().push('error', e instanceof Error ? e.message : String(e), 6000)
    }
  }

  const w = hw.data
  const installed = (models.data ?? []).filter((m) => m.installed)

  return (
    <div className="flex flex-col gap-4" data-testid="settings-hardware">
      <RuntimeCard />
      <Card title={t('settings.hardware.title')} testId="settings-worker">
        <Row label={t('settings.hardware.worker')} hint={w?.error ?? undefined}>
          {w ? (
            <span className="flex items-center gap-2">
              <span className={`h-2 w-2 rounded-full ${w.state === 'ready' ? 'bg-success' : w.state === 'busy' || w.state === 'starting' ? 'bg-ai' : 'bg-faint'}`} />
              {t(`worker.state_${w.state}`)}
            </span>
          ) : (
            <Loader2 size={14} className="animate-spin text-muted" />
          )}
        </Row>
        <Row label={t('settings.hardware.tier')} hint={t('settings.hardware.tierHint')}>
          <span className="rounded bg-accent/20 px-2 py-0.5 font-semibold text-accent" data-testid="settings-tier">
            {w?.tier ?? '—'}
          </span>
        </Row>
        <Row label={t('settings.hardware.device')} hint={w?.providers.join(', ')}>
          <span className="text-right">{w?.gpu ? `${w.gpu.name} · ${(w.gpu.vram_mb / 1024).toFixed(0)} GB` : (w?.device ?? 'CPU')}</span>
        </Row>
        <Row label={t('settings.assistant.engine')} hint={t('settings.assistant.engineHint', { llm: status.data?.llm_model ?? '—', vlm: status.data?.vlm_model ?? '—' })}>
          {s && (
            <Select
              label={t('settings.assistant.engine')}
              testId="setting-assistant-engine"
              value={s.assistant.engine}
              options={ENGINES.map((e) => ({ value: e, label: t(`settings.assistant.engine_${e}`) }))}
              onChange={(v) => set('assistant.engine', v)}
            />
          )}
        </Row>
      </Card>

      <Card title={t('settings.models.title')} hint={t('settings.models.hint')} testId="settings-models">
        <Row label={t('settings.models.source')} hint={t('settings.models.sourceHint')}>
          {s && (
            <Select
              label={t('settings.models.source')}
              testId="setting-models-source"
              value={s.models.source}
              options={SOURCES.map((v) => ({ value: v, label: t(`settings.models.source_${v}`) }))}
              onChange={(v) => set('models.source', v)}
            />
          )}
        </Row>
        <Row label={t('settings.models.dir')} hint={t('settings.models.installedSummary', { n: installed.length, size: mb(installed.reduce((n, m) => n + m.size_mb, 0)) })}>
          <code className="max-w-72 truncate text-xs text-muted" title={s?.models.dir}>
            {s?.models.dir || '—'}
          </code>
        </Row>
        {offline && (
          <div className="flex items-start gap-2 bg-warning/10 px-4 py-2 text-xs text-warning" data-testid="offline-hint">
            <AlertTriangle size={14} className="mt-0.5 shrink-0" />
            {t('settings.models.offline')}
          </div>
        )}
        <div className="overflow-x-auto">
          <table className="w-full min-w-[560px] text-left" data-testid="models-table">
            <thead className="text-xs text-muted">
              <tr>
                <th className="px-4 py-2 font-medium">{t('settings.models.col_model')}</th>
                <th className="px-2 py-2 font-medium">{t('settings.models.col_size')}</th>
                <th className="px-2 py-2 font-medium">{t('settings.models.col_license')}</th>
                <th className="px-2 py-2 font-medium">{t('settings.models.col_status')}</th>
                <th className="px-4 py-2" />
              </tr>
            </thead>
            <tbody>
              {models.isPending && (
                <tr>
                  <td colSpan={5} className="px-4 py-3 text-muted">
                    <Loader2 size={14} className="inline animate-spin" /> {t('library.loading')}
                  </td>
                </tr>
              )}
              {(models.data ?? []).map((m) => {
                const task = tasks[m.id] ? taskState[tasks[m.id]] : undefined
                const busy = task?.state === 'running'
                const pct = task && task.total > 0 ? Math.round((task.done / task.total) * 100) : 0
                return (
                  <tr key={m.id} className="border-t border-line" data-testid="model-row" data-model={m.id} data-installed={m.installed}>
                    <td className="px-4 py-2">
                      <div className="font-medium">{m.id}</div>
                      <div className="text-xs text-muted">{m.task.map((k) => t(`models.task_${k}`, { defaultValue: k })).join(' · ')}</div>
                    </td>
                    <td className="tnum px-2 py-2 whitespace-nowrap text-muted">{mb(m.size_mb)}</td>
                    <td className="px-2 py-2">
                      <span
                        className={`inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-xs ${m.noncommercial ? 'bg-warning/20 font-semibold text-warning' : 'bg-bg text-muted'}`}
                        title={m.noncommercial ? t('models.ncHint') : m.license}
                        data-nc={m.noncommercial}
                      >
                        {m.noncommercial && <AlertTriangle size={11} />}
                        {m.license}
                        {m.noncommercial && ` · ${t('models.nc')}`}
                      </span>
                    </td>
                    <td className="px-2 py-2 whitespace-nowrap">
                      {busy ? (
                        <span className="flex min-w-24 flex-col gap-1" data-testid="model-progress">
                          <span className="tnum text-xs text-muted">
                            {formatBytes(task?.done ?? 0)} / {formatBytes(task?.total ?? 0)}
                          </span>
                          <span className="h-1.5 overflow-hidden rounded bg-line">
                            <span className="block h-full bg-ai transition-[width] duration-200" style={{ width: `${pct}%` }} />
                          </span>
                        </span>
                      ) : m.installed ? (
                        <span className="flex items-center gap-1 text-success">
                          <Check size={13} />
                          {t('settings.models.installed')}
                        </span>
                      ) : (
                        <span className="text-muted">{t('settings.models.notInstalled')}</span>
                      )}
                    </td>
                    <td className="px-4 py-2 text-right whitespace-nowrap">
                      {m.installed ? (
                        <button className="btn btn-ghost btn-icon" aria-label={t('settings.models.delete')} title={t('settings.models.delete')} onClick={() => setAskDelete(m)} data-testid="model-delete">
                          <Trash2 size={14} />
                        </button>
                      ) : (
                        <button
                          className="btn"
                          disabled={busy || offline}
                          onClick={() => (m.noncommercial ? setAskDownload(m) : void download(m))}
                          title={offline ? t('settings.models.offline') : undefined}
                          data-testid="model-download"
                        >
                          {busy ? <Loader2 size={13} className="animate-spin" /> : <Download size={13} />}
                          {t('settings.models.download')}
                        </button>
                      )}
                    </td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        </div>
      </Card>

      <ConfirmDialog
        open={askDownload !== null}
        onOpenChange={(o) => !o && setAskDownload(null)}
        title={t('settings.models.ncTitle', { id: askDownload?.id })}
        description={t('models.ncWarning')}
        confirmLabel={t('settings.models.download')}
        onConfirm={() => {
          const m = askDownload
          setAskDownload(null)
          if (m) void download(m)
        }}
      />
      <ConfirmDialog
        open={askDelete !== null}
        onOpenChange={(o) => !o && setAskDelete(null)}
        title={t('settings.models.deleteTitle', { id: askDelete?.id })}
        description={t('settings.models.deleteDesc', { size: mb(askDelete?.size_mb ?? 0) })}
        confirmLabel={t('settings.models.delete')}
        danger
        testId="model-delete-confirm"
        onConfirm={() => {
          const m = askDelete
          setAskDelete(null)
          if (m) void remove(m)
        }}
      />
    </div>
  )
}
