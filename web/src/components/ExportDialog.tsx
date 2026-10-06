import { useMutation } from '@tanstack/react-query'
import { FolderOpen, Loader2 } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { api } from '@/api/client'
import { useToasts } from '@/stores/toasts'
import { FolderBrowser } from './FolderBrowser'
import { Modal } from './Modal'

interface Props {
  open: boolean
  onOpenChange: (b: boolean) => void
  selectedIds: number[]
  allIds: number[]
  /** Export by person: sub-folder name -> photo ids (replaces the scope choice; docs/api-contract-m4.md C.2). */
  folders?: Record<string, number[]>
}

type EdgeChoice = 'original' | '2048' | '4096' | 'custom'

const LS_KEY = 'imagepicker.exportDest'
const readDest = () => {
  try {
    return localStorage.getItem(LS_KEY) ?? ''
  } catch {
    return ''
  }
}

export function ExportDialog({ open, onOpenChange, selectedIds, allIds, folders }: Props) {
  const { t } = useTranslation()
  const [dest, setDest] = useState(readDest)
  const [edge, setEdge] = useState<EdgeChoice>('2048')
  const [custom, setCustom] = useState(3000)
  const [quality, setQuality] = useState(90)
  const [template, setTemplate] = useState('{name}')
  const [scope, setScope] = useState<'selected' | 'all'>('selected')
  const [browse, setBrowse] = useState(false)
  const push = useToasts((s) => s.push)
  const updateTask = useToasts((s) => s.updateTask)

  const hasSelection = selectedIds.length > 0
  const effectiveScope = hasSelection ? scope : 'all'
  const byFolder = folders && Object.keys(folders).length > 0 ? folders : undefined
  const ids = byFolder ? Object.values(byFolder).flat() : effectiveScope === 'selected' ? selectedIds : allIds
  const longEdge = edge === 'original' ? null : edge === 'custom' ? custom : Number(edge)

  const mut = useMutation({
    mutationFn: () => api.exportPhotos({ ...(byFolder ? { folders: byFolder } : { ids }), dest, long_edge: longEdge, quality, name_template: template || '{name}' }),
    onSuccess: ({ task_id }) => {
      try {
        localStorage.setItem(LS_KEY, dest)
      } catch {
        /* ignore */
      }
      // Show the toast immediately; task.progress events will update it.
      updateTask({ type: 'task.progress', task_id, kind: 'export', done: 0, total: ids.length, state: 'running' })
      onOpenChange(false)
    },
    onError: (e) => push('error', e instanceof Error ? e.message : String(e)),
  })

  const example = (template || '{name}').replaceAll('{name}', 'IMG_0001') + '.jpg'

  return (
    <>
      <Modal
        open={open}
        onOpenChange={onOpenChange}
        title={t('export.title')}
        footer={
          <>
            <button className="btn" onClick={() => onOpenChange(false)}>
              {t('common.cancel')}
            </button>
            <button className="btn btn-primary" disabled={!dest.trim() || ids.length === 0 || mut.isPending} onClick={() => mut.mutate()}>
              {mut.isPending && <Loader2 size={14} className="animate-spin" />}
              {t('export.start', { n: ids.length })}
            </button>
          </>
        }
      >
        <div className="flex flex-col gap-4">
          {byFolder && (
            <div className="flex flex-col gap-1" data-testid="export-folders">
              <span className="text-muted">{t('export.byPerson')}</span>
              <ul className="max-h-32 overflow-y-auto rounded-control border border-line p-1.5 text-xs">
                {Object.entries(byFolder).map(([name, list]) => (
                  <li key={name} className="flex justify-between gap-2 px-1 py-0.5">
                    <span className="truncate font-mono">{name}/</span>
                    <span className="tnum text-muted">{t('home.photoCount', { n: list.length })}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}
          <fieldset className={`flex gap-4 ${byFolder ? 'hidden' : ''}`}>
            <legend className="mb-1 text-muted">{t('export.scope')}</legend>
            <label className="flex items-center gap-1.5">
              <input type="radio" name="scope" checked={effectiveScope === 'selected'} disabled={!hasSelection} onChange={() => setScope('selected')} />
              {t('export.selected', { n: selectedIds.length })}
            </label>
            <label className="flex items-center gap-1.5">
              <input type="radio" name="scope" checked={effectiveScope === 'all'} onChange={() => setScope('all')} />
              {t('export.allShown', { n: allIds.length })}
            </label>
          </fieldset>

          <div className="flex flex-col gap-1">
            <label htmlFor="exp-dest" className="text-muted">
              {t('export.dest')}
            </label>
            <div className="flex gap-2">
              <input id="exp-dest" className="field flex-1 font-mono text-xs" value={dest} onChange={(e) => setDest(e.target.value)} placeholder={t('export.destPlaceholder')} spellCheck={false} />
              <button className="btn" onClick={() => setBrowse(true)}>
                <FolderOpen size={14} />
                {t('export.browse')}
              </button>
            </div>
          </div>

          <div className="flex flex-col gap-1">
            <label htmlFor="exp-edge" className="text-muted">
              {t('export.longEdge')}
            </label>
            <div className="flex items-center gap-2">
              <select id="exp-edge" className="field" value={edge} onChange={(e) => setEdge(e.target.value as EdgeChoice)}>
                <option value="original">{t('export.original')}</option>
                <option value="2048">2048 px</option>
                <option value="4096">4096 px</option>
                <option value="custom">{t('export.custom')}</option>
              </select>
              {edge === 'custom' && (
                <input type="number" className="field w-28" min={64} max={16384} value={custom} onChange={(e) => setCustom(Math.max(64, Number(e.target.value) || 64))} aria-label={t('export.custom')} />
              )}
            </div>
          </div>

          <div className="flex flex-col gap-1">
            <label htmlFor="exp-q" className="text-muted">
              {t('export.quality')}: <span className="tnum text-fg">{quality}</span>
            </label>
            <input id="exp-q" type="range" min={50} max={100} value={quality} onChange={(e) => setQuality(Number(e.target.value))} />
          </div>

          <div className="flex flex-col gap-1">
            <label htmlFor="exp-tpl" className="text-muted">
              {t('export.template')}
            </label>
            <input id="exp-tpl" className="field font-mono text-xs" value={template} onChange={(e) => setTemplate(e.target.value)} spellCheck={false} />
            <div className="text-xs text-faint">
              {t('export.templateHint')} <span className="tnum text-muted">{example}</span>
            </div>
          </div>
        </div>
      </Modal>
      <FolderBrowser open={browse} onOpenChange={setBrowse} title={t('export.dest')} onSelect={setDest} />
    </>
  )
}
