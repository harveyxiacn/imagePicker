import { useQueryClient } from '@tanstack/react-query'
import { Check, ChevronLeft, Eye, EyeOff, Loader2, Merge, Pencil, Users } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { api, faceCropUrl } from '@/api/client'
import { usePeople, useSession } from '@/api/queries'
import type { Person } from '@/api/types'
import { Modal } from '@/components/Modal'
import { HeaderControls } from '@/components/HeaderControls'
import { qk } from '@/lib/cache'
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
  const push = useToasts((s) => s.push)

  const list = useMemo(() => (people.data ?? []).filter((p) => showHidden || !p.hidden), [people.data, showHidden])
  const hiddenCount = (people.data ?? []).filter((p) => p.hidden).length
  const chosen = list.filter((p) => selected.has(p.id))

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
          <button className="btn btn-primary" disabled={chosen.length < 2} onClick={startMerge} data-testid="people-merge">
            <Merge size={14} />
            {t('people.merge', { n: chosen.length })}
          </button>
          <HeaderControls />
        </div>
      </header>

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
