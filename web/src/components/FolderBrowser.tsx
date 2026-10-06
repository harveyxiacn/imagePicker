import { useQuery } from '@tanstack/react-query'
import { ArrowUp, Folder, HardDrive, Image as ImageIcon, Loader2 } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { api } from '@/api/client'
import { fileBaseName, resolveDirEntry } from '@/lib/format'
import { Modal } from './Modal'

interface Props {
  open: boolean
  onOpenChange: (open: boolean) => void
  onSelect: (path: string) => void
  title?: string
  confirmLabel?: string
}

/** Server-side folder browser backed by /api/fs/*. Used in both browser and Tauri modes for now. */
export function FolderBrowser({ open, onOpenChange, onSelect, title, confirmLabel }: Props) {
  const { t } = useTranslation()
  const [path, setPathRaw] = useState<string | undefined>(undefined)
  const [edited, setDraft] = useState<string | null>(null)
  const setPath = (p: string | undefined) => {
    setPathRaw(p)
    setDraft(null)
  }

  const roots = useQuery({ queryKey: ['fs', 'roots'], queryFn: api.fsRoots, enabled: open, staleTime: 60_000 })
  const list = useQuery({
    queryKey: ['fs', 'list', path ?? ''],
    queryFn: () => api.fsList(path),
    enabled: open,
    staleTime: 10_000,
    retry: false,
  })

  const current = list.data
  const draft = edited ?? current?.path ?? path ?? ''

  return (
    <Modal
      open={open}
      onOpenChange={onOpenChange}
      title={title ?? t('folder.title')}
      width="max-w-xl"
      footer={
        <>
          <button className="btn" onClick={() => onOpenChange(false)}>
            {t('common.cancel')}
          </button>
          <button
            className="btn btn-primary"
            disabled={!current}
            onClick={() => {
              if (current) {
                onSelect(current.path)
                onOpenChange(false)
              }
            }}
          >
            {confirmLabel ?? t('folder.choose')}
          </button>
        </>
      }
    >
      <div className="flex flex-col gap-3">
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
          <button
            type="button"
            className="btn btn-icon"
            disabled={!current?.parent}
            aria-label={t('folder.up')}
            title={t('folder.up')}
            onClick={() => current?.parent && setPath(current.parent)}
          >
            <ArrowUp size={14} />
          </button>
          <input
            className="field flex-1 font-mono text-xs"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            aria-label={t('folder.path')}
            spellCheck={false}
          />
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
            <div className="p-4 text-danger">{(list.error as Error).message}</div>
          ) : current && current.dirs.length === 0 ? (
            <div className="p-4 text-muted">{t('folder.empty')}</div>
          ) : (
            <ul>
              {current?.dirs.map((d) => {
                const full = resolveDirEntry(current.path, d)
                return (
                  <li key={full}>
                    <button
                      className="flex w-full items-center gap-2 px-3 py-1.5 text-left hover:bg-elevated"
                      onDoubleClick={() => setPath(full)}
                      onClick={() => setPath(full)}
                    >
                      <Folder size={15} className="shrink-0 text-accent" />
                      <span className="truncate">{fileBaseName(full)}</span>
                    </button>
                  </li>
                )
              })}
            </ul>
          )}
        </div>
        {current && (
          <div className="flex items-center gap-1.5 text-muted">
            <ImageIcon size={14} />
            {t('folder.imageCount', { n: current.image_count })}
          </div>
        )}
      </div>
    </Modal>
  )
}
