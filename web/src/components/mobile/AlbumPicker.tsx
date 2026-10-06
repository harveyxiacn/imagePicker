import { useQuery } from '@tanstack/react-query'
import { FolderOpen, ImageOff, Loader2, Lock, RefreshCw, ShieldAlert } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { getMobilePlatform, type Album, type MediaPermission } from '@/lib/mobilePlatform'
import { Sheet } from './Sheet'

interface Props {
  open: boolean
  onOpenChange: (open: boolean) => void
  onSelect: (album: Album) => void
  /** Fallback: open the folder browser instead. */
  onBrowse: () => void
}

interface Result {
  perm: MediaPermission
  albums: Album[]
}

/** Resolve permission, then list albums. A denied plugin may also fail the listing: both mean "denied". */
async function load(retry: boolean): Promise<Result> {
  const mp = getMobilePlatform()
  let perm: MediaPermission = 'granted'
  if (mp.requestPermission) perm = await mp.requestPermission(retry)
  if (perm === 'denied' || !mp.listAlbums) return { perm, albums: [] }
  try {
    return { perm, albums: await mp.listAlbums() }
  } catch (e) {
    if (/permission|denied|403/i.test(e instanceof Error ? e.message : String(e))) return { perm: 'denied', albums: [] }
    throw e
  }
}

function formatDay(ms: number | null | undefined, lang: string): string {
  return ms ? new Date(ms).toLocaleDateString(lang, { month: 'short', day: 'numeric' }) : ''
}

/** Mobile import: album list from the platform media plugin (`list_albums`) with permission states. */
export function AlbumPicker({ open, onOpenChange, onSelect, onBrowse }: Props) {
  const { t, i18n } = useTranslation()
  const [attempt, setAttempt] = useState(0)
  const q = useQuery({ queryKey: ['m8', 'albums', attempt], queryFn: () => load(attempt > 0), enabled: open, retry: false, staleTime: 0, gcTime: 0 })
  const mp = getMobilePlatform()
  const perm = q.data?.perm

  return (
    <Sheet open={open} onOpenChange={onOpenChange} title={t('mobile.albums.title')} tall testId="album-sheet">
      {q.isPending ? (
        <div className="flex items-center justify-center gap-2 p-10 text-muted" data-testid="album-loading">
          <Loader2 className="animate-spin" size={18} />
          {t('mobile.albums.loading')}
        </div>
      ) : q.isError ? (
        <div className="flex flex-col items-center gap-3 p-8 text-center" data-testid="album-error">
          <div className="text-danger">{(q.error as Error).message}</div>
          <button className="btn" onClick={() => setAttempt((n) => n + 1)}>
            <RefreshCw size={14} />
            {t('common.retry')}
          </button>
        </div>
      ) : perm === 'denied' ? (
        <div className="flex flex-col items-center gap-3 p-8 text-center" data-testid="album-denied">
          <Lock size={36} className="text-warning" />
          <div className="text-base font-semibold">{t('mobile.albums.deniedTitle')}</div>
          <p className="max-w-xs text-muted">{t('mobile.albums.deniedBody')}</p>
          <button className="btn btn-primary" onClick={() => setAttempt((n) => n + 1)} data-testid="album-retry">
            <RefreshCw size={14} />
            {t('mobile.albums.grant')}
          </button>
          <button className="btn btn-ghost" onClick={onBrowse}>
            <FolderOpen size={14} />
            {t('mobile.albums.useFolder')}
          </button>
        </div>
      ) : (
        <div data-testid="album-list">
          {perm === 'partial' && (
            <div className="flex items-start gap-2 border-b border-line bg-warning/10 p-3 text-xs" data-testid="album-partial">
              <ShieldAlert size={16} className="mt-0.5 shrink-0 text-warning" />
              <div className="flex-1">
                <div className="font-medium">{t('mobile.albums.partialTitle')}</div>
                <div className="text-muted">{t('mobile.albums.partialBody')}</div>
              </div>
              <button className="btn" onClick={() => setAttempt((n) => n + 1)}>
                {t('mobile.albums.allowAll')}
              </button>
            </div>
          )}
          {q.data?.albums.length === 0 ? (
            <div className="p-8 text-center text-muted">{t('mobile.albums.empty')}</div>
          ) : (
            <ul className="divide-y divide-line">
              {q.data?.albums.map((a) => {
                const cover = mp.coverUrl(a)
                return (
                  <li key={a.id}>
                    <button className="flex min-h-[72px] w-full items-center gap-3 px-4 py-2 text-left active:bg-elevated" onClick={() => onSelect(a)} data-testid={`album-${a.id}`}>
                      <div className="flex h-14 w-14 shrink-0 items-center justify-center overflow-hidden rounded-lg bg-bg">
                        {cover ? <img src={cover} alt="" className="h-full w-full object-cover" draggable={false} /> : <ImageOff size={20} className="text-faint" />}
                      </div>
                      <div className="min-w-0 flex-1">
                        <div className="truncate text-base font-medium">{a.name}</div>
                        <div className="truncate text-xs text-faint">{a.path}</div>
                      </div>
                      <div className="text-right">
                        <div className="tnum text-base font-semibold">{a.count.toLocaleString(i18n.language)}</div>
                        <div className="tnum text-xs text-muted">{formatDay(a.latest_ms, i18n.language)}</div>
                      </div>
                    </button>
                  </li>
                )
              })}
            </ul>
          )}
          <div className="p-3">
            <button className="btn w-full justify-center" onClick={onBrowse}>
              <FolderOpen size={14} />
              {t('mobile.albums.useFolder')}
            </button>
          </div>
        </div>
      )}
    </Sheet>
  )
}
