import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Check, ChevronLeft, Download, Eye, EyeOff, LayoutGrid, Loader2, Merge, Pencil, ScanFace, Star, Trophy, Users } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { api, faceCropUrl, fetchAllPhotos, thumbUrl } from '@/api/client'
import { usePeople, useSession } from '@/api/queries'
import type { Person } from '@/api/types'
import { ExportDialog } from '@/components/ExportDialog'
import { FaceSearchDialog } from '@/components/FaceSearchDialog'
import { Modal } from '@/components/Modal'
import { HeaderControls } from '@/components/HeaderControls'
import { buildBestSections, sectionsToFolders } from '@/lib/bestN'
import { qk } from '@/lib/cache'
import { applyProfilesToPhotos } from '@/lib/editActions'
import { DEFAULT_PERSON_FILTER } from '@/lib/filter'
import { useToasts } from '@/stores/toasts'
import { useUi } from '@/stores/ui'

function NameField({ person, onSave }: { person: Person; onSave: (name: string | null) => void }) {
  const { t } = useTranslation()
  const [editing, setEditing] = useState(false)
  const [value, setValue] = useState('')
  const commit = () => {
    setEditing(false)
    const v = value.trim()
    if ((v || null) !== person.name) onSave(v || null)
  }
  if (editing) {
    return (
      <input
        autoFocus
        className="field w-full"
        value={value}
        placeholder={t('face.namePlaceholder')}
        aria-label={t('people.rename')}
        onChange={(e) => setValue(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === 'Enter') commit()
          if (e.key === 'Escape') setEditing(false)
        }}
        data-testid="person-name-input"
      />
    )
  }
  return (
    <button
      className="group/name flex w-full items-center gap-1 rounded px-1 text-left hover:bg-panel"
      title={t('people.rename')}
      onClick={() => {
        setValue(person.name ?? '')
        setEditing(true)
      }}
      data-testid="person-name"
    >
      <span className={`truncate font-medium ${person.name ? '' : 'text-muted italic'}`}>{person.name ?? `${t('person.unnamed')} ${person.id}`}</span>
      <Pencil size={12} className="shrink-0 text-faint opacity-0 group-hover/name:opacity-100" />
    </button>
  )
}

/** People page (doc 04 3.6): face cards with inline rename, multi-select merge, hide, click to filter the library. */
export function People() {
  const { sessionId: raw } = useParams()
  const sessionId = Number(raw)
  const { t, i18n } = useTranslation()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const session = useSession(sessionId)
  const people = usePeople(sessionId)
  const [selected, setSelected] = useState<ReadonlySet<number>>(new Set())
  const [showHidden, setShowHidden] = useState(false)
  const [mergeOpen, setMergeOpen] = useState(false)
  const [into, setInto] = useState<number | null>(null)
  const [view, setView] = useState<'cards' | 'best'>('cards')
  const [bestN, setBestN] = useState(3)
  const [exportFolders, setExportFolders] = useState<Record<string, number[]> | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const push = useToasts((s) => s.push)

  const list = useMemo(() => (people.data ?? []).filter((p) => showHidden || !p.hidden), [people.data, showHidden])
  const hiddenCount = (people.data ?? []).filter((p) => p.hidden).length
  const chosen = list.filter((p) => selected.has(p.id))

  // "Best N per person" works on the selection, or on everyone shown when nothing is selected.
  const bestIds = useMemo(() => (chosen.length ? chosen : list).map((p) => p.id), [chosen, list])
  const best = useQuery({
    queryKey: qk.best(sessionId, bestIds, bestN),
    queryFn: () => api.bestPeople(sessionId, bestIds, bestN),
    enabled: view === 'best' && bestIds.length > 0,
    staleTime: 30_000,
  })
  const sections = useMemo(() => buildBestSections(list, best.data, bestIds, bestN), [list, best.data, bestIds, bestN])

  const refresh = () => void qc.invalidateQueries({ queryKey: qk.peopleAll })
  const fail = (err: unknown) => push('error', err instanceof Error ? err.message : String(err))

  const patch = async (p: Person, body: { name?: string | null; hidden?: boolean }) => {
    // optimistic
    qc.setQueryData<{ people: Person[] }>(qk.people(sessionId), (old) =>
      old ? { people: old.people.map((x) => (x.id === p.id ? { ...x, ...body } : x)) } : old,
    )
    try {
      await api.patchPerson(p.id, body)
      refresh()
    } catch (err) {
      fail(err)
      refresh()
    }
  }

  const openLibrary = (p: Person) => {
    useUi.getState().openWithFilter({ person: { ...DEFAULT_PERSON_FILTER, include: [p.id] } })
    navigate(`/s/${sessionId}`)
  }

  /** "View photos together": library filtered with persons AND. */
  const openTogether = () => {
    useUi.getState().openWithFilter({ person: { ...DEFAULT_PERSON_FILTER, include: chosen.map((p) => p.id), mode: 'all' } })
    navigate(`/s/${sessionId}`)
  }

  /** Export by person: best N of every chosen person into a sub-folder named after them. */
  const exportByPerson = async () => {
    setBusy('export')
    try {
      const ids = bestIds
      const resp = await qc.fetchQuery({ queryKey: qk.best(sessionId, ids, bestN), queryFn: () => api.bestPeople(sessionId, ids, bestN), staleTime: 30_000 })
      const folders = sectionsToFolders(buildBestSections(list, resp, ids, bestN))
      if (Object.keys(folders).length === 0) push('info', t('best.empty'), 2500)
      else setExportFolders(folders)
    } catch (err) {
      fail(err)
    } finally {
      setBusy(null)
    }
  }

  /** Apply the saved beauty profiles of the chosen people to every photo they appear in. */
  const applyProfiles = async () => {
    setBusy('profiles')
    try {
      const all = await fetchAllPhotos({ session_id: sessionId, persons: chosen.map((p) => p.id), person_mode: 'any', include_background: true })
      const ids = all.photos.map((p) => p.id)
      const n = await applyProfilesToPhotos(qc, ids, t('history.applyProfiles', { n: ids.length }))
      push(n > 0 ? 'success' : 'info', n > 0 ? t('beauty.profilesApplied', { n }) : t('beauty.profilesNone'), 3000)
    } catch (err) {
      fail(err)
    } finally {
      setBusy(null)
    }
  }

  const toggle = (id: number) =>
    setSelected((s) => {
      const n = new Set(s)
      if (n.has(id)) n.delete(id)
      else n.add(id)
      return n
    })

  const startMerge = () => {
    setInto([...chosen].sort((a, b) => b.photo_count - a.photo_count)[0]?.id ?? null)
    setMergeOpen(true)
  }

  const doMerge = async () => {
    if (into === null) return
    setMergeOpen(false)
    try {
      await api.mergePeople(chosen.map((p) => p.id), into)
      setSelected(new Set())
      refresh()
      push('success', t('people.merged', { n: chosen.length }), 2500)
    } catch (err) {
      fail(err)
    }
  }

  return (
    <div className="flex h-full flex-col" data-testid="people-page">
      <header className="flex h-12 shrink-0 items-center gap-3 border-b border-line px-3">
        <Link to={`/s/${sessionId}`} className="btn btn-ghost" aria-label={t('common.back')}>
          <ChevronLeft size={16} />
          <span className="hidden max-w-48 truncate font-semibold sm:inline">{session.data?.title ?? '…'}</span>
        </Link>
        <h1 className="flex items-center gap-2 text-base font-semibold">
          <Users size={16} />
          {t('people.title')}
        </h1>
        <span className="tnum text-muted">{t('people.count', { n: list.length })}</span>
        <div className="ml-auto flex items-center gap-2">
          {hiddenCount > 0 && (
            <button className="btn" aria-pressed={showHidden} onClick={() => setShowHidden((v) => !v)}>
              {showHidden ? <Eye size={14} /> : <EyeOff size={14} />}
              {t('people.showHidden', { n: hiddenCount })}
            </button>
          )}
          <button className="btn" onClick={() => useUi.getState().setFaceSearchOpen(true)} data-testid="people-face-search">
            <span aria-hidden>📷</span> {t('faceSearch.button')}
          </button>
          <button className="btn btn-primary" disabled={chosen.length < 2} onClick={startMerge} data-testid="people-merge">
            <Merge size={14} />
            {t('people.merge', { n: chosen.length })}
          </button>
          <HeaderControls />
        </div>
      </header>

      <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-line px-3 py-1.5" data-testid="people-toolbar">
        <div className="flex rounded-control border border-line p-0.5" role="tablist" aria-label={t('people.view')}>
          {(['cards', 'best'] as const).map((v) => (
            <button
              key={v}
              role="tab"
              aria-selected={view === v}
              className={`flex h-6 items-center gap-1.5 rounded px-2 transition-colors ${view === v ? 'bg-accent text-accent-fg' : 'text-muted hover:text-fg'}`}
              onClick={() => setView(v)}
              data-testid={`people-view-${v}`}
            >
              {v === 'cards' ? <LayoutGrid size={13} /> : <Trophy size={13} />}
              {t(`people.view_${v}`)}
            </button>
          ))}
        </div>
        <label className="flex items-center gap-1.5">
          <span className="text-muted">{t('best.perPerson')}</span>
          <select className="field !px-1.5" value={bestN} onChange={(e) => setBestN(Number(e.target.value))} aria-label={t('best.count')} data-testid="best-n">
            {[1, 3, 5, 10].map((n) => (
              <option key={n} value={n}>
                {t('best.n', { n })}
              </option>
            ))}
          </select>
        </label>
        <span className="mx-1 h-4 border-l border-line" />
        <span className="tnum text-muted" data-testid="people-selected">
          {t('people.selected', { n: chosen.length })}
        </span>
        <button className="btn" disabled={chosen.length < 2} onClick={openTogether} title={chosen.length < 2 ? t('people.togetherNeedsTwo') : undefined} data-testid="people-together">
          <Users size={13} />
          {t('people.together')}
        </button>
        <button className="btn" disabled={busy !== null || list.length === 0} onClick={() => void exportByPerson()} data-testid="people-export">
          {busy === 'export' ? <Loader2 size={13} className="animate-spin" /> : <Download size={13} />}
          {t('people.exportByPerson')}
        </button>
        <button className="btn" disabled={busy !== null || chosen.length === 0} onClick={() => void applyProfiles()} data-testid="people-apply-profiles">
          {busy === 'profiles' ? <Loader2 size={13} className="animate-spin" /> : <ScanFace size={13} />}
          {t('beauty.applyProfilesToSelectedPeople')}
        </button>
      </div>

      <main className="min-h-0 flex-1 overflow-y-auto p-4">
        {people.isPending ? (
          <div className="flex h-full items-center justify-center gap-2 text-muted">
            <Loader2 className="animate-spin" size={18} />
            {t('library.loading')}
          </div>
        ) : people.isError ? (
          <div className="flex h-full flex-col items-center justify-center gap-3 text-danger">
            {(people.error as Error).message}
            <button className="btn" onClick={() => void people.refetch()}>
              {t('common.retry')}
            </button>
          </div>
        ) : list.length === 0 ? (
          <div className="flex h-full flex-col items-center justify-center gap-3 text-center text-muted" data-testid="people-empty">
            <Users size={36} className="text-faint" />
            <div>{t('people.empty')}</div>
            <Link to={`/s/${sessionId}`} className="btn">
              {t('people.goAnalyze')}
            </Link>
          </div>
        ) : view === 'best' ? (
          <div className="flex flex-col gap-6" data-testid="best-view">
            {best.isPending && (
              <div className="flex items-center justify-center gap-2 py-10 text-muted">
                <Loader2 className="animate-spin" size={18} />
                {t('library.loading')}
              </div>
            )}
            {best.isError && <div className="text-danger">{(best.error as Error).message}</div>}
            {best.isSuccess && sections.length === 0 && <div className="py-10 text-center text-muted">{t('best.empty')}</div>}
            {sections.map((sec) => (
              <section key={sec.person.id} data-testid="best-section" aria-label={sec.person.name ?? String(sec.person.id)}>
                <div className="mb-2 flex items-center gap-3">
                  <img src={faceCropUrl(sec.person.cover_face_id, 128)} alt="" className="h-9 w-9 rounded-full object-cover" draggable={false} />
                  <div className="min-w-0">
                    <div className="truncate font-medium">{sec.person.name ?? `${t('person.unnamed')} ${sec.person.id}`}</div>
                    <div className="tnum text-xs text-muted">{t('best.shown', { n: sec.photos.length, total: sec.person.photo_count })}</div>
                  </div>
                  <button className="btn btn-ghost ml-auto" onClick={() => openLibrary(sec.person)}>
                    {t('people.openLibrary')}
                  </button>
                </div>
                <div className="grid gap-2" style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(140px, 1fr))' }}>
                  {sec.photos.map((ph, i) => (
                    <Link
                      key={ph.photoId}
                      to={`/s/${sessionId}/edit/${ph.photoId}`}
                      className="group relative block aspect-[4/3] overflow-hidden rounded-control border border-line bg-bg"
                      title={t('best.open')}
                      data-testid="best-photo"
                    >
                      <img src={thumbUrl({ id: ph.photoId, thumb_version: 'b' }, 256)} alt="" className="h-full w-full object-cover transition-transform group-hover:scale-105" loading="lazy" draggable={false} />
                      <span className="tnum absolute bottom-1 left-1 flex items-center gap-0.5 rounded bg-black/65 px-1 text-[11px] leading-4 text-white">
                        {i === 0 && <Star size={10} className="text-accent" fill="currentColor" />}
                        {Math.round(ph.score * 100)}
                      </span>
                    </Link>
                  ))}
                </div>
              </section>
            ))}
          </div>
        ) : (
          <div className="grid gap-4" style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(150px, 1fr))' }} data-testid="people-grid">
            {list.map((p) => {
              const on = selected.has(p.id)
              return (
                <div
                  key={p.id}
                  className="group relative flex flex-col gap-2 rounded-card border bg-panel p-2"
                  style={{ borderColor: on ? 'var(--accent)' : 'var(--line)', opacity: p.hidden ? 0.55 : 1 }}
                  data-testid="person-card"
                >
                  <button className="relative block aspect-square w-full overflow-hidden rounded-control bg-bg" onClick={() => openLibrary(p)} title={t('people.openLibrary')} aria-label={`${t('people.openLibrary')}: ${p.name ?? p.id}`}>
                    <img src={faceCropUrl(p.cover_face_id, 256)} alt="" className="h-full w-full object-cover transition-transform group-hover:scale-105" draggable={false} />
                  </button>
                  <button
                    className="absolute top-3 left-3 flex h-5 w-5 items-center justify-center rounded border border-white/60 bg-black/55 text-white"
                    style={{ background: on ? 'var(--accent)' : undefined, color: on ? 'var(--accent-fg)' : undefined }}
                    aria-pressed={on}
                    aria-label={t('people.select')}
                    onClick={() => toggle(p.id)}
                    data-testid="person-select"
                  >
                    {on && <Check size={13} strokeWidth={3} />}
                  </button>
                  <button
                    className="btn btn-icon absolute top-3 right-3 !h-6 !w-6 bg-black/55 opacity-0 backdrop-blur group-hover:opacity-100 focus-visible:opacity-100"
                    onClick={() => void patch(p, { hidden: !p.hidden })}
                    aria-label={p.hidden ? t('people.unhide') : t('people.hide')}
                    title={p.hidden ? t('people.unhide') : t('people.hide')}
                    data-testid="person-hide"
                  >
                    {p.hidden ? <Eye size={13} /> : <EyeOff size={13} />}
                  </button>
                  <NameField person={p} onSave={(name) => void patch(p, { name })} />
                  <div className="tnum px-1 text-xs text-muted">{t('home.photoCount', { n: p.photo_count.toLocaleString(i18n.language) })}</div>
                </div>
              )
            })}
          </div>
        )}
      </main>

      <FaceSearchDialog sessionId={sessionId} />
      <ExportDialog open={exportFolders !== null} onOpenChange={(o) => !o && setExportFolders(null)} selectedIds={[]} allIds={[]} folders={exportFolders ?? undefined} />
      <Modal
        open={mergeOpen}
        onOpenChange={setMergeOpen}
        title={t('people.mergeTitle')}
        description={t('people.mergeDesc')}
        width="max-w-md"
        footer={
          <>
            <button className="btn" onClick={() => setMergeOpen(false)}>
              {t('common.cancel')}
            </button>
            <button className="btn btn-primary" onClick={() => void doMerge()} data-testid="people-merge-confirm">
              {t('people.mergeConfirm')}
            </button>
          </>
        }
      >
        <div className="flex flex-col gap-1.5" role="radiogroup" aria-label={t('people.mergeInto')}>
          {chosen.map((p) => (
            <label key={p.id} className="flex cursor-pointer items-center gap-3 rounded-control border border-line p-2" style={{ borderColor: into === p.id ? 'var(--accent)' : undefined }}>
              <input type="radio" name="merge-into" checked={into === p.id} onChange={() => setInto(p.id)} />
              <img src={faceCropUrl(p.cover_face_id, 128)} alt="" className="h-9 w-9 rounded-full object-cover" />
              <span className="flex-1 truncate">{p.name ?? `${t('person.unnamed')} ${p.id}`}</span>
              <span className="tnum text-xs text-muted">{p.photo_count}</span>
            </label>
          ))}
        </div>
      </Modal>
    </div>
  )
}
