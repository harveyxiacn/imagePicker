import { useQueryClient } from '@tanstack/react-query'
import { FileUp, Save, Trash2, X } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { api } from '@/api/client'
import { useLuts, usePhotoPresets } from '@/api/queries'
import type { EditStack, Preset } from '@/api/types'
import { qk } from '@/lib/cache'
import { appliedPreset, applyPreset, getLut, PRESET_AMOUNT_MAX, scalePresetStack, setLut, stackKey } from '@/lib/edit'
import { changeStack, commitLive, setLive } from '@/lib/editActions'
import { cachedThumb, renderThumb } from '@/lib/thumbQueue'
import { useEdit } from '@/stores/edit'
import { useToasts } from '@/stores/toasts'
import { Section } from './Section'
import { Slider } from './Slider'

/** Built-in LUT ids understood by the renderer (`{type:"lut", file}`); imported ones are added at runtime. */

function PresetCard({ photoId, preset, label, active, onApply, onDelete }: { photoId: number; preset: Preset; label: string; active: boolean; onApply: () => void; onDelete?: () => void }) {
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
      <button
        className={`block w-full overflow-hidden rounded-control border text-left hover:border-accent ${active ? 'border-accent' : 'border-line'}`}
        onClick={onApply}
        data-testid={`preset-${preset.id}`}
        aria-pressed={active}
        title={preset.builtin ? `${label} · ${t('edit.presetAdaptiveTip')}` : label}
      >
        <div className="aspect-[3/2] bg-bg">{url ? <img src={url} alt="" className="h-full w-full object-cover" draggable={false} /> : <div className="skeleton h-full w-full" />}</div>
        <div className="truncate px-1.5 py-0.5 text-[11px]">{label}</div>
      </button>
      {onDelete && (
        <button
          className="btn btn-icon absolute top-0.5 right-0.5 !h-5 !w-5 bg-black/60 opacity-0 group-hover:opacity-100 [@media(hover:none)]:opacity-100"
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
  const luts = useLuts().data ?? []
  const lut = getLut(stack)
  const [path, setPath] = useState('')
  const [busy, setBusy] = useState(false)

  const choose = (file: string) => changeStack(qc, setLut(useEdit.getState().stack, file ? { file, amount: lut?.amount ?? 1 } : null), t('history.lut'))
  const doImport = async () => {
    if (!path.trim()) return
    setBusy(true)
    try {
      const res = await api.importLut(path.trim())
      await qc.invalidateQueries({ queryKey: qk.luts })
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
          {luts.map((l) => (
            <option key={l.id} value={l.id}>
              {l.builtin ? t(l.name) : l.name}
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

/**
 * Strength of the applied preset (0–150 %). Dragging scales the fitted 100 % look in the browser
 * for the live preview; on release the server fits the preset again at that strength (highlight
 * protection included), so the stored values are the exact ones.
 */
function PresetStrength({ photoId, preset, label }: { photoId: number; preset: Preset; label: string }) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const stack = useEdit((s) => s.stack)
  const amount = appliedPreset(stack)?.amount ?? 1
  // While dragging, the slider shows the dragged value (at 0 % the look, and its provenance, is gone).
  const drag = useRef<number | null>(null)
  const [shown, setShown] = useState<number | null>(null)
  const live = (v: number) => {
    drag.current = v
    setShown(v)
    setLive(applyPreset(useEdit.getState().stack, retag(scalePresetStack(preset.stack, v / 100), preset.id, v)))
  }
  const commit = async () => {
    const v = drag.current ?? Math.round(amount * 100)
    drag.current = null
    try {
      const res = await api.applyPresetFor(photoId, preset.id, v / 100)
      if (useEdit.getState().photoId !== photoId) return
      setLive(applyPreset(useEdit.getState().stack, res.stack))
    } catch (err) {
      useToasts.getState().push('error', err instanceof Error ? err.message : String(err), 5000)
    }
    setShown(null)
    commitLive(qc, t('history.presetStrength', { name: label }), 'preset:amount')
  }
  return (
    <div className="mt-2" data-testid="preset-strength">
      <Slider
        testId="preset-amount"
        label={t('edit.presetStrength')}
        value={shown ?? Math.round(amount * 100)}
        min={0}
        max={Math.round(PRESET_AMOUNT_MAX * 100)}
        step={5}
        def={100}
        unit="%"
        onLive={live}
        onCommit={() => void commit()}
      />
      {preset.builtin && <div className="text-[11px] text-muted">{t('edit.presetAdaptiveHint')}</div>}
    </div>
  )
}

/** Set the `source` of the global op to `preset:<id>;amount=<v>`. */
function retag(s: EditStack, id: string, v: number): EditStack {
  return { ...s, ops: s.ops.map((o) => (o.type === 'global' ? { ...o, source: `preset:${id};amount=${Math.round(v)}` } : o)) }
}

/** Presets (built-in + user, thumbnails fitted to this photo) and LUT. */
export function PresetsPanel({ photoId }: { photoId: number }) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const presets = usePhotoPresets(photoId)
  const stack = useEdit((s) => s.stack)
  const [name, setName] = useState('')

  const label = (p: Preset) => (p.builtin ? t(p.name, { defaultValue: p.id }) : p.name)
  // The listed stacks are already fitted to this photo at 100 %.
  const apply = (p: Preset) => changeStack(qc, applyPreset(useEdit.getState().stack, p.stack), t('history.preset', { name: label(p) }))

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
  const activeId = appliedPreset(stack)?.id ?? null
  const active = list.find((p) => p.id === activeId)
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
              active={activeId === p.id}
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
      {active && <PresetStrength key={active.id} photoId={photoId} preset={active} label={label(active)} />}
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
