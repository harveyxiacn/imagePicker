import { useQueryClient } from '@tanstack/react-query'
import { FileUp, Save, Trash2, X } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { api } from '@/api/client'
import { usePresets } from '@/api/queries'
import type { Preset } from '@/api/types'
import { qk } from '@/lib/cache'
import { applyPreset, getLut, setLut, stackKey } from '@/lib/edit'
import { changeStack, commitLive, setLive } from '@/lib/editActions'
import { cachedThumb, renderThumb } from '@/lib/thumbQueue'
import { useEdit } from '@/stores/edit'
import { useToasts } from '@/stores/toasts'
import { Section } from './Section'
import { Slider } from './Slider'

/** Built-in LUT ids understood by the renderer (`{type:"lut", file}`); imported ones are added at runtime. */
const BUILTIN_LUTS = ['film_warm', 'film_cool', 'teal_orange', 'matte', 'bw']

function PresetCard({ photoId, preset, label, onApply, onDelete }: { photoId: number; preset: Preset; label: string; onApply: () => void; onDelete?: () => void }) {
  const { t } = useTranslation()
  const key = `${photoId}|${preset.id}|${stackKey(preset.stack)}`
  const [loaded, setLoaded] = useState<{ key: string; url: string } | null>(() => {
    const url = cachedThumb(key)
    return url ? { key, url } : null
  })
  useEffect(() => {
    let alive = true
    renderThumb(key, { photo_id: photoId, stack: preset.stack, long_edge: 128 })
      .then((url) => alive && setLoaded({ key, url }))
      .catch(() => undefined)
    return () => {
      alive = false
    }
  }, [key, photoId, preset.stack])
  const url = loaded?.key === key ? loaded.url : null
  return (
    <div className="group relative">
      <button className="block w-full overflow-hidden rounded-control border border-line text-left hover:border-accent" onClick={onApply} data-testid={`preset-${preset.id}`} title={label}>
        <div className="aspect-[3/2] bg-bg">{url ? <img src={url} alt="" className="h-full w-full object-cover" draggable={false} /> : <div className="skeleton h-full w-full" />}</div>
        <div className="truncate px-1.5 py-0.5 text-[11px]">{label}</div>
      </button>
      {onDelete && (
        <button
          className="btn btn-icon absolute top-0.5 right-0.5 !h-5 !w-5 bg-black/60 opacity-0 group-hover:opacity-100"
          aria-label={t('edit.deletePreset')}
          onClick={onDelete}
        >
          <Trash2 size={11} />
        </button>
      )}
    </div>
  )
}

/** LUT selector + amount + .cube import. */
function LutBlock() {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const stack = useEdit((s) => s.stack)
  const imported = useEdit((s) => s.importedLuts)
  const lut = getLut(stack)
  const [path, setPath] = useState('')
  const [busy, setBusy] = useState(false)

  const choose = (file: string) => changeStack(qc, setLut(useEdit.getState().stack, file ? { file, amount: lut?.amount ?? 1 } : null), t('history.lut'))
  const doImport = async () => {
    if (!path.trim()) return
    setBusy(true)
    try {
      const res = await api.importLut(path.trim())
      useEdit.getState().addLut(res)
      setPath('')
      choose(res.id)
      useToasts.getState().push('success', t('edit.lutImported', { name: res.name }), 2500)
    } catch (err) {
      useToasts.getState().push('error', err instanceof Error ? err.message : String(err), 5000)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="mt-3 border-t border-line/60 pt-2" data-testid="lut-block">
      <div className="mb-1 text-[11px] font-semibold tracking-wide text-muted uppercase">{t('edit.lut')}</div>
      <div className="flex items-center gap-1.5">
        <select className="field min-w-0 flex-1" value={lut?.file ?? ''} onChange={(e) => choose(e.target.value)} aria-label={t('edit.lut')} data-testid="lut-select">
          <option value="">{t('edit.lutNone')}</option>
          {BUILTIN_LUTS.map((id) => (
            <option key={id} value={id}>
              {t(`lut.${id}`)}
            </option>
          ))}
          {imported.map((l) => (
            <option key={l.id} value={l.id}>
              {l.name}
            </option>
          ))}
        </select>
        {lut && (
          <button className="btn btn-ghost btn-icon" aria-label={t('edit.lutNone')} onClick={() => choose('')}>
            <X size={13} />
          </button>
        )}
      </div>
      {lut && (
        <Slider
          testId="lut-amount"
          label={t('edit.amount')}
          value={Math.round((lut.amount ?? 1) * 100)}
          min={0}
          max={100}
          step={1}
          def={100}
          unit="%"
          onLive={(v) => setLive(setLut(useEdit.getState().stack, { file: lut.file, amount: v / 100 }))}
          onCommit={() => commitLive(qc, t('history.lutAmount'), 'lut:amount')}
        />
      )}
      {/* TODO(desktop): native file picker via the platform adapter; the WebUI has no way to browse local files. */}
      <div className="mt-1.5 flex gap-1.5">
        <input
          className="field min-w-0 flex-1"
          placeholder={t('edit.lutPath')}
          value={path}
          aria-label={t('edit.lutPath')}
          data-testid="lut-path"
          onChange={(e) => setPath(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && void doImport()}
        />
        <button className="btn" disabled={busy || !path.trim()} onClick={() => void doImport()} data-testid="lut-import">
          <FileUp size={13} />
          {t('edit.lutImport')}
        </button>
      </div>
    </div>
  )
}

/** Presets (built-in + user, live thumbnails) and LUT. */
export function PresetsPanel({ photoId }: { photoId: number }) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const presets = usePresets()
  const [name, setName] = useState('')

  const label = (p: Preset) => (p.builtin ? t(p.name, { defaultValue: p.id }) : p.name)
  const apply = (p: Preset) =>
    changeStack(qc, applyPreset(useEdit.getState().stack, p.stack), t('history.preset', { name: label(p) }))

  const save = async () => {
    const n = name.trim()
    if (!n) return
    try {
      await api.savePreset(n, useEdit.getState().stack)
      setName('')
      void qc.invalidateQueries({ queryKey: qk.presets })
      useToasts.getState().push('success', t('edit.presetSaved', { name: n }), 2200)
    } catch (err) {
      useToasts.getState().push('error', err instanceof Error ? err.message : String(err), 5000)
    }
  }

  const list = presets.data ?? []
  return (
    <Section id="presets" title={t('edit.presets')}>
      {presets.isError ? (
        <div className="text-xs text-danger">{(presets.error as Error).message}</div>
      ) : (
        <div className="grid grid-cols-3 gap-1.5" data-testid="preset-grid">
          {list.map((p) => (
            <PresetCard
              key={p.id}
              photoId={photoId}
              preset={p}
              label={label(p)}
              onApply={() => apply(p)}
              onDelete={
                p.builtin
                  ? undefined
                  : () =>
                      void api
                        .deletePreset(p.id)
                        .then(() => qc.invalidateQueries({ queryKey: qk.presets }))
                        .catch((err: unknown) => useToasts.getState().push('error', err instanceof Error ? err.message : String(err)))
              }
            />
          ))}
          {presets.isPending && Array.from({ length: 6 }, (_, i) => <div key={i} className="skeleton aspect-[3/2] rounded-control" />)}
        </div>
      )}
      <div className="mt-2 flex gap-1.5">
        <input
          className="field min-w-0 flex-1"
          placeholder={t('edit.presetName')}
          aria-label={t('edit.presetName')}
          value={name}
          data-testid="preset-name"
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && void save()}
        />
        <button className="btn" disabled={!name.trim()} onClick={() => void save()} data-testid="preset-save">
          <Save size={13} />
          {t('edit.presetSave')}
        </button>
      </div>
      <LutBlock />
    </Section>
  )
}
