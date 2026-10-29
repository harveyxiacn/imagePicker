import { useQuery } from '@tanstack/react-query'
import { ArrowUp, File as FileIcon, Folder, HardDrive, Loader2 } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { api } from '@/api/client'
import { errorText } from '@/lib/errors'
import { fileBaseName, resolveDirEntry } from '@/lib/format'
import { Modal } from './Modal'

interface Props {
  open: boolean
  onOpenChange: (open: boolean) => void
  /** Called with the full path of the chosen file. */
  onSelect: (path: string) => void
  /** File extensions offered, without the dot (`['png']`). */
  exts: string[]
  title: string
}

/** Server-side file chooser backed by /api/fs/list (inside the allowed folders), like [FolderBrowser]. */
export function FilePicker({ open, onOpenChange, onSelect, exts, title }: Props) {
  const { t } = useTranslation()
  const [path, setPathRaw] = useState<string | undefined>(undefined)
  const [edited, setDraft] = useState<string | null>(null)
  const setPath = (p: string | undefined) => {
    setPathRaw(p)
    setDraft(null)
  }
  const roots = useQuery({ queryKey: ['fs', 'roots'], queryFn: api.fsRoots, enabled: open, staleTime: 60_000 })
  const list = useQuery({
    queryKey: ['fs', 'list', path ?? '', exts.join(',')],
    queryFn: () => api.fsList(path, exts),
    enabled: open,
    staleTime: 10_000,
    retry: false,
  })
  const current = list.data
  const draft = edited ?? current?.path ?? path ?? ''
  const files = current?.files ?? []

  return (
    <Modal
      open={open}
      onOpenChange={onOpenChange}
      title={title}
      width="max-w-xl"
      footer={
        <button className="btn" onClick={() => onOpenChange(false)}>
          {t('common.cancel')}
        </button>
      }
    >
      <div className="flex flex-col gap-3" data-testid="file-picker">
        <div className="flex flex-wrap gap-1.5">
          {roots.data?.roots.map((r) => (
            <button key={r} className="btn" onClick={() => setPath(r)} title={r}>
              <HardDrive size={14} />
              {r.length > 14 ? fileBaseName(r) : r}
            </button>
          ))}
        </div>
        <form
          className="flex gap-2"
          onSubmit={(e) => {
            e.preventDefault()
            setPath(draft || undefined)
          }}
        >
          <button type="button" className="btn btn-icon" disabled={!current?.parent} aria-label={t('folder.up')} title={t('folder.up')} onClick={() => current?.parent && setPath(current.parent)}>
            <ArrowUp size={14} />
          </button>
          <input className="field flex-1 font-mono text-xs" value={draft} onChange={(e) => setDraft(e.target.value)} aria-label={t('folder.path')} spellCheck={false} />
          <button className="btn" type="submit">
            {t('folder.go')}
          </button>
        </form>
        <div className="h-72 overflow-auto rounded-control border border-line bg-bg">
          {list.isPending ? (
            <div className="flex h-full items-center justify-center text-muted">
              <Loader2 className="animate-spin" size={18} />
            </div>
          ) : list.isError ? (
            <div className="p-4 text-danger">{errorText(list.error)}</div>
          ) : current && current.dirs.length === 0 && files.length === 0 ? (
            <div className="p-4 text-muted">{t('filePicker.empty', { exts: exts.join(', ') })}</div>
          ) : (
            <ul>
              {current?.dirs.map((d) => {
                const full = resolveDirEntry(current.path, d)
                return (
                  <li key={full}>
                    <button className="flex w-full items-center gap-2 px-3 py-1.5 text-left hover:bg-elevated" onClick={() => setPath(full)}>
                      <Folder size={15} className="shrink-0 text-accent-text" />
                      <span className="truncate">{fileBaseName(full)}</span>
                    </button>
                  </li>
                )
              })}
              {files.map((f) => (
                <li key={f}>
                  <button
                    className="flex w-full items-center gap-2 px-3 py-1.5 text-left hover:bg-elevated"
                    data-testid="file-picker-file"
                    onClick={() => {
                      onSelect(f)
                      onOpenChange(false)
                    }}
                  >
                    <FileIcon size={15} className="shrink-0 text-muted" />
                    <span className="truncate">{fileBaseName(f)}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </Modal>
  )
}
