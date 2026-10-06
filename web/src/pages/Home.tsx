import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Aperture, CheckCircle2, FolderOpen, HeartHandshake, ImageOff, Images, Loader2, Settings, Trash2, Upload } from 'lucide-react'
import { useMemo, useState, type DragEvent } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useNavigate } from 'react-router-dom'
import { api, thumbUrl } from '@/api/client'
import { useMe, useSessions } from '@/api/queries'
import type { Session } from '@/api/types'
import { FolderBrowser } from '@/components/FolderBrowser'
import { AlbumPicker } from '@/components/mobile/AlbumPicker'
import { HeaderControls } from '@/components/HeaderControls'
import { UserMenu } from '@/components/UserMenu'
import { can } from '@/lib/auth'
import { errorText } from '@/lib/errors'
import { qk } from '@/lib/cache'
import { getMobilePlatform } from '@/lib/mobilePlatform'
import { useIsMobile } from '@/lib/useLayout'
import { getPlatform, useNativeImport } from '@/platform'
import { useToasts } from '@/stores/toasts'

export function Home() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const qc = useQueryClient()
  const sessions = useSessions()
  const [browse, setBrowse] = useState(false)
  const [albumsOpen, setAlbumsOpen] = useState(false)
  const isMobile = useIsMobile()
  const mobilePlatform = useMemo(() => getMobilePlatform(), [])
  const [dragOver, setDragOver] = useState(false)
  const platform = useMemo(() => getPlatform(), [])
  const push = useToasts((s) => s.push)
  const role = useMe().data?.role ?? 'owner'

  const importMut = useMutation({
    mutationFn: (v: string | { path: string; title?: string }) => api.importFolder(typeof v === 'string' ? { path: v } : v),
    onSuccess: ({ session }) => {
      void qc.invalidateQueries({ queryKey: qk.sessions })
      navigate(`/s/${session.id}`)
    },
    onError: (e) => push('error', errorText(e), 9000),
  })

  // Desktop shell: menu "Import Folder…" and OS folder drops arrive as native events.
  useNativeImport((p) => importMut.mutate(p), setDragOver)

  const chooseFolder = async () => {
    if (!platform.pickFolder) return setBrowse(true)
    try {
      const p = await platform.pickFolder()
      if (p) importMut.mutate(p)
    } catch (e) {
      push('error', e instanceof Error ? e.message : String(e))
    }
  }

  const onDrop = (e: DragEvent) => {
    e.preventDefault()
    setDragOver(false)
    if (!platform.canDropFolders) push('info', t('home.dropNeedsDesktop'), 6000)
  }

  return (
    <div className="flex h-full flex-col">
      <header className="flex h-12 shrink-0 items-center justify-between border-b border-line px-4">
        <div className="flex items-center gap-2 text-base font-semibold">
          <Aperture size={20} className="text-accent" />
          imagePicker
        </div>
        <div className="flex items-center gap-2">
          <HeaderControls />
          <Link to="/taste" className="btn btn-ghost" data-testid="home-taste-link" aria-label={t('taste.title')}>
            <HeartHandshake size={15} />
            <span className="max-md:hidden">{t('taste.title')}</span>
          </Link>
          {can(role, 'settings') && (
            <Link to="/settings" className="btn btn-ghost max-md:!hidden" data-testid="home-settings-link">
              <Settings size={15} />
              {t('home.settings')}
            </Link>
          )}
          <UserMenu />
        </div>
      </header>

      <main className="min-h-0 flex-1 overflow-auto">
        <div className="mx-auto flex max-w-5xl flex-col gap-8 p-4 sm:p-8">
          {can(role, 'edit') && <section
            onDragOver={(e) => {
              e.preventDefault()
              setDragOver(true)
            }}
            onDragLeave={() => setDragOver(false)}
            onDrop={onDrop}
            className={`flex flex-col items-center gap-3 rounded-card border-2 border-dashed px-6 py-10 text-center transition-colors ${
              dragOver ? 'border-accent bg-accent/10' : 'border-line bg-panel'
            }`}
            aria-label={t('home.dropTitle')}
          >
            <Upload size={32} className="text-muted" />
            <div className="text-base font-medium">{isMobile && mobilePlatform.listAlbums ? t('mobile.albums.cta') : t('home.dropTitle')}</div>
            <div className="flex flex-wrap justify-center gap-2">
              {mobilePlatform.listAlbums && (
                <button className="btn btn-primary !h-9 px-4" onClick={() => setAlbumsOpen(true)} disabled={importMut.isPending} data-testid="import-albums">
                  <Images size={15} />
                  {t('mobile.albums.title')}
                </button>
              )}
              <button className={`btn !h-9 px-4 ${mobilePlatform.listAlbums ? '' : 'btn-primary'}`} onClick={() => void chooseFolder()} disabled={importMut.isPending}>
                {importMut.isPending ? <Loader2 size={15} className="animate-spin" /> : <FolderOpen size={15} />}
                {t('home.chooseFolder')}
              </button>
              {!mobilePlatform.listAlbums && (
                <button className="btn !h-9" disabled title={t('common.soon')}>
                  {t('home.importDevice')}
                </button>
              )}
            </div>
            {!platform.canDropFolders && !isMobile && <div className="text-xs text-faint">{t('home.browserHint')}</div>}
          </section>}

          <section>
            <h2 className="mb-3 text-base font-semibold">{t('home.recent')}</h2>
            {sessions.isPending ? (
              <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-4">
                {[0, 1, 2].map((i) => (
                  <div key={i} className="skeleton h-56 rounded-card" />
                ))}
              </div>
            ) : sessions.isError ? (
              <div className="rounded-card border border-line bg-panel p-4 text-danger">
                {t('home.loadFailed')}: {(sessions.error as Error).message}
              </div>
            ) : sessions.data.length === 0 ? (
              <div className="rounded-card border border-line bg-panel p-8 text-center text-muted">{t('home.empty')}</div>
            ) : (
              <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-4">
                {sessions.data.map((s) => (
                  <SessionCard key={s.id} s={s} />
                ))}
              </div>
            )}
          </section>
        </div>
      </main>

      <AlbumPicker
        open={albumsOpen}
        onOpenChange={setAlbumsOpen}
        onBrowse={() => {
          setAlbumsOpen(false)
          setBrowse(true)
        }}
        onSelect={(a) => {
          setAlbumsOpen(false)
          importMut.mutate({ path: a.path, title: a.name })
        }}
      />
      <FolderBrowser
        open={browse}
        onOpenChange={setBrowse}
        title={t('home.chooseFolder')}
        confirmLabel={t('home.importThis')}
        onSelect={(p) => importMut.mutate(p)}
      />
    </div>
  )
}

function SessionCard({ s }: { s: Session }) {
  const { t, i18n } = useTranslation()
  const qc = useQueryClient()
  const del = useMutation({
    mutationFn: () => api.deleteSession(s.id),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.sessions }),
  })
  const importing = s.import_state !== 'ready'
  return (
    <div className="group relative overflow-hidden rounded-card border border-line bg-panel transition-colors hover:border-faint">
      <Link to={`/s/${s.id}`} className="block">
        <div className="relative aspect-[4/3] bg-bg">
          {s.cover_photo_id !== null ? (
            <img
              src={thumbUrl({ id: s.cover_photo_id, thumb_version: '' }, 256)}
              alt=""
              className="h-full w-full object-cover"
              loading="lazy"
              draggable={false}
            />
          ) : (
            <div className="flex h-full items-center justify-center text-faint">
              <ImageOff size={28} />
            </div>
          )}
        </div>
        <div className="flex flex-col gap-1 p-3">
          <div className="truncate text-base font-medium">{s.title}</div>
          <div className="tnum text-muted">{t('home.photoCount', { n: s.photo_count.toLocaleString(i18n.language) })}</div>
          <div className="tnum flex items-center gap-2 text-xs text-muted">
            <span>{t('home.picked', { n: s.picked_count })}</span>
            <span>·</span>
            <span>{t('home.rated', { n: s.rated_count })}</span>
          </div>
          <div className="mt-0.5 flex items-center gap-1 text-xs">
            {importing ? (
              <>
                <Loader2 size={12} className="animate-spin text-accent" />
                <span className="text-accent">{t(`import.${s.import_state}`)}</span>
              </>
            ) : (
              <>
                <CheckCircle2 size={12} className="text-success" />
                <span className="text-success">{t('import.ready')}</span>
              </>
            )}
          </div>
        </div>
      </Link>
      <button
        className="btn btn-icon absolute top-2 right-2 bg-black/60 opacity-0 backdrop-blur group-hover:opacity-100 focus-visible:opacity-100 [@media(hover:none)]:opacity-100"
        aria-label={t('home.remove')}
        title={t('home.removeHint')}
        onClick={() => {
          if (confirm(t('home.removeConfirm', { title: s.title }))) del.mutate()
        }}
      >
        <Trash2 size={14} />
      </button>
    </div>
  )
}
