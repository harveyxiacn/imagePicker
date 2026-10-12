import { useQueryClient } from '@tanstack/react-query'
import { Check, ImagePlus, Loader2, ScanSearch, UserPlus } from 'lucide-react'
import { useEffect, useMemo, useReducer, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useMatch, useNavigate } from 'react-router-dom'
import { api, ApiError, faceCropUrl } from '@/api/client'
import { usePeople } from '@/api/queries'
import { qk } from '@/lib/cache'
import { faceSearchReducer, firstImage, initFaceSearch, newPersonFaceIds, queryPhoto, similarityPct, type FaceSearchEvent } from '@/lib/faceSearch'
import { DEFAULT_PERSON_FILTER } from '@/lib/filter'
import { useToasts } from '@/stores/toasts'
import { useUi } from '@/stores/ui'
import { Modal } from './Modal'

/**
 * Search by face (F2.13.4): drop / pick / paste a photo (or start from a library face via the face menu), choose a
 * detected face, see the matching people with similarity; clicking one filters the library by that person.
 * "This is a new person" moves the ticked similar faces (and a library query face) to a new, locked person.
 */
export function FaceSearchDialog({ sessionId }: { sessionId: number }) {
  const { t } = useTranslation()
  const open = useUi((s) => s.faceSearchOpen)
  const setOpen = useUi((s) => s.setFaceSearchOpen)
  const seed = useUi((s) => s.faceSearchFace)
  return (
    <Modal open={open} onOpenChange={setOpen} title={t('faceSearch.title')} description={seed ? t('faceSearch.descFace') : t('faceSearch.desc')} width="max-w-2xl">
      {/* keyed by the query face so the state machine starts from it */}
      <FaceSearchBody key={seed?.id ?? 'upload'} sessionId={sessionId} seed={seed} close={() => setOpen(false)} />
    </Modal>
  )
}

function FaceSearchBody({ sessionId, seed, close }: { sessionId: number; seed: { id: number; photo_id: number } | null; close: () => void }) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const [state, dispatch] = useReducer(faceSearchReducer, seed, initFaceSearch)
  const [over, setOver] = useState(false)
  /** Name typed in the "this is a new person" step; null while that step is not shown. */
  const [naming, setNaming] = useState<string | null>(null)
  const [creating, setCreating] = useState(false)
  const input = useRef<HTMLInputElement>(null)
  const people = usePeople(sessionId)
  const navigate = useNavigate()
  const inLibrary = useMatch('/s/:sessionId')

  /** A new query (or face) leaves the naming step. */
  const go = (e: FaceSearchEvent) => {
    setNaming(null)
    dispatch(e)
  }

  // Run the request whenever the machine enters `searching`.
  useEffect(() => {
    if (state.status !== 'searching') return
    let alive = true
    const q = state.query
    const req =
      q.kind === 'image'
        ? api.faceSearch({ image: q.file }, { session_id: sessionId, ...(state.faceIndex !== null ? { face_index: state.faceIndex } : {}) })
        : api.faceSearch({ face_id: q.faceId }, { session_id: sessionId })
    req
      .then((resp) => alive && dispatch({ type: 'result', resp }))
      .catch((err: unknown) => alive && dispatch({ type: 'fail', message: err instanceof ApiError || err instanceof Error ? err.message : String(err) }))
    return () => {
      alive = false
    }
  }, [state, sessionId])

  // Paste an image from the clipboard while the dialog is open.
  useEffect(() => {
    const onPaste = (e: ClipboardEvent) => {
      const file = firstImage(e.clipboardData?.files)
      if (file) {
        e.preventDefault()
        setNaming(null)
        dispatch({ type: 'pick', file })
      }
    }
    document.addEventListener('paste', onPaste)
    return () => document.removeEventListener('paste', onPaste)
  }, [])

  const query = state.status === 'idle' ? null : state.query
  const file = query?.kind === 'image' ? query.file : null
  const previewUrl = useMemo(() => (file ? URL.createObjectURL(file) : null), [file])
  useEffect(() => () => void (previewUrl && URL.revokeObjectURL(previewUrl)), [previewUrl])

  // boxes only make sense on an uploaded photo (a library face is shown as its crop)
  const faces = query?.kind !== 'image' ? [] : state.status === 'results' ? state.resp.faces_detected : state.status === 'searching' ? state.faces : []
  const selected = state.status === 'results' ? state.faceIndex : state.status === 'searching' ? state.faceIndex : null
  const coverOf = (personId: number) => people.data?.find((p) => p.id === personId)?.cover_face_id
  const results = state.status === 'results' ? state : null
  const lockedPhoto = query ? queryPhoto(query) : null
  const newIds = results ? newPersonFaceIds(results.query, results.resp.similar_faces, results.chosen) : []

  const pickPerson = (personId: number) => {
    const person = { ...DEFAULT_PERSON_FILTER, include: [personId] }
    if (inLibrary) useUi.getState().setFilter({ person })
    else {
      useUi.getState().openWithFilter({ person })
      navigate(`/s/${sessionId}`)
    }
    close()
  }

  /** "This is a new person": create it from the chosen faces, then show its photos in the library. */
  const createPerson = async () => {
    if (newIds.length === 0 || naming === null || creating) return
    setCreating(true)
    try {
      const { person } = await api.createPerson(newIds, naming.trim() || null)
      void qc.invalidateQueries({ queryKey: qk.peopleAll })
      void qc.invalidateQueries({ queryKey: ['analysis', 'photo'] })
      void qc.invalidateQueries({ queryKey: ['burstFaces'] })
      void qc.invalidateQueries({ queryKey: qk.photoPeopleAll })
      useToasts.getState().push('success', t('faceSearch.created', { name: person.name ?? `${t('person.unnamed')} ${person.id}` }), 3000)
      if (inLibrary) useUi.getState().setFilter({ person: { ...DEFAULT_PERSON_FILTER, include: [person.id] } })
      close()
    } catch (err) {
      useToasts.getState().push('error', err instanceof Error ? err.message : String(err))
      setCreating(false)
    }
  }

  return (
    <div className="flex flex-col gap-4" data-testid="face-search">
      {state.status === 'idle' ? (
        <div
          className="flex h-48 cursor-pointer flex-col items-center justify-center gap-2 rounded-card border-2 border-dashed text-center text-muted transition-colors"
          style={{ borderColor: over ? 'var(--accent)' : 'var(--line)', background: over ? 'color-mix(in srgb, var(--accent) 8%, transparent)' : undefined }}
          onClick={() => input.current?.click()}
          onDragOver={(e) => {
            e.preventDefault()
            setOver(true)
          }}
          onDragLeave={() => setOver(false)}
          onDrop={(e) => {
            e.preventDefault()
            setOver(false)
            const f = firstImage(e.dataTransfer.files)
            if (f) go({ type: 'pick', file: f })
          }}
          role="button"
          tabIndex={0}
          onKeyDown={(e) => (e.key === 'Enter' || e.key === ' ') && input.current?.click()}
          data-testid="face-search-drop"
        >
          <ImagePlus size={30} className="text-faint" />
          <div>{t('faceSearch.drop')}</div>
          <div className="text-xs text-faint">{t('faceSearch.dropHint')}</div>
        </div>
      ) : (
        <div className="flex flex-col gap-4 sm:flex-row">
          <div className="flex shrink-0 flex-col gap-2 sm:w-56">
            <div className="relative overflow-hidden rounded-card border border-line bg-bg" data-testid="face-search-preview">
              {previewUrl && <img src={previewUrl} alt={t('faceSearch.query')} className="block w-full" draggable={false} />}
              {query?.kind === 'face' && <img src={faceCropUrl(query.faceId, 256)} alt={t('faceSearch.queryFace')} className="block aspect-square w-full object-cover" draggable={false} />}
              {faces.map((f, i) => (
                <button
                  key={i}
                  type="button"
                  className="absolute rounded-sm border-2 transition-colors"
                  style={{
                    left: `${f[0] * 100}%`,
                    top: `${f[1] * 100}%`,
                    width: `${f[2] * 100}%`,
                    height: `${f[3] * 100}%`,
                    borderColor: i === selected ? 'var(--accent)' : 'rgba(255,255,255,.8)',
                    boxShadow: i === selected ? '0 0 0 1px rgba(0,0,0,.5)' : undefined,
                  }}
                  aria-label={t('faceSearch.pickFace', { n: i + 1 })}
                  aria-pressed={i === selected}
                  disabled={state.status !== 'results'}
                  onClick={() => go({ type: 'selectFace', index: i })}
                  data-testid="detected-face"
                >
                  <span className="absolute -top-px -left-px rounded-br bg-black/70 px-1 text-[10px] leading-4 text-white">{i + 1}</span>
                </button>
              ))}
            </div>
            <button className="btn btn-ghost" onClick={() => go({ type: 'reset' })} data-testid="face-search-again">
              <ImagePlus size={13} />
              {t('faceSearch.another')}
            </button>
            {faces.length > 1 && <div className="text-[11px] text-muted">{t('faceSearch.multiFaces', { n: faces.length })}</div>}
          </div>

          <div className="min-w-0 flex-1">
            {state.status === 'searching' && (
              <div className="flex h-full min-h-32 items-center justify-center gap-2 text-muted" data-testid="face-search-busy">
                <Loader2 size={16} className="animate-spin" />
                {t('faceSearch.searching')}
              </div>
            )}
            {state.status === 'noFace' && (
              <div className="flex h-full min-h-32 items-center justify-center text-center text-muted" data-testid="face-search-noface">
                {t('faceSearch.noFace')}
              </div>
            )}
            {state.status === 'error' && <div className="text-danger">{state.message}</div>}
            {results && (
              <div className="flex flex-col gap-2" data-testid="face-search-results">
                <div className="text-xs text-muted">{t('faceSearch.candidates')}</div>
                {results.resp.candidates.length === 0 ? (
                  <div className="py-4 text-center text-muted">{t('faceSearch.noMatch')}</div>
                ) : (
                  <ul className="flex flex-col gap-1.5">
                    {results.resp.candidates.map((c) => {
                      const cover = coverOf(c.person_id)
                      const pct = similarityPct(c.similarity)
                      return (
                        <li key={c.person_id}>
                          <button
                            className="flex w-full items-center gap-3 rounded-control border border-line p-2 text-left transition-colors hover:border-accent hover:bg-elevated"
                            onClick={() => pickPerson(c.person_id)}
                            title={t('faceSearch.filterByThis')}
                            data-testid="face-candidate"
                          >
                            {cover !== undefined ? (
                              <img src={faceCropUrl(cover, 128)} alt="" className="h-10 w-10 shrink-0 rounded-full object-cover" draggable={false} />
                            ) : (
                              <span className="h-10 w-10 shrink-0 rounded-full bg-bg" />
                            )}
                            <span className="min-w-0 flex-1">
                              <span className="block truncate font-medium">{c.person_name ?? `${t('person.unnamed')} ${c.person_id}`}</span>
                              <span className="mt-1 block h-1.5 overflow-hidden rounded bg-line">
                                <span className="block h-full bg-accent" style={{ width: `${pct}%` }} />
                              </span>
                            </span>
                            <span className="tnum w-11 shrink-0 text-right text-sm" data-testid="candidate-similarity">
                              {pct}%
                            </span>
                          </button>
                        </li>
                      )
                    })}
                  </ul>
                )}

                <div className="mt-1 flex flex-col gap-1.5 border-t border-line pt-2" data-testid="face-search-similar">
                  <div className="flex items-center justify-between gap-2 text-xs text-muted">
                    <span>{t('faceSearch.similarCount', { n: results.resp.similar_faces.length })}</span>
                    {results.resp.similar_faces.length > 0 && <span className="truncate text-faint">{t('faceSearch.tickHint')}</span>}
                  </div>
                  {(results.query.kind === 'face' || results.resp.similar_faces.length > 0) && (
                    <ul className="grid grid-cols-5 gap-1.5">
                      {results.query.kind === 'face' && (
                        <li title={t('faceSearch.queryFace')} data-testid="similar-face-query">
                          <div className="relative overflow-hidden rounded-control border-2 border-accent">
                            <img src={faceCropUrl(results.query.faceId, 128)} alt={t('faceSearch.queryFace')} className="block aspect-square w-full object-cover" draggable={false} />
                            <span className="absolute top-0.5 left-0.5 flex h-4 w-4 items-center justify-center rounded bg-accent text-accent-fg">
                              <Check size={11} strokeWidth={3} />
                            </span>
                          </div>
                        </li>
                      )}
                      {results.resp.similar_faces.map((f) => {
                        const on = results.chosen.has(f.face_id)
                        const blocked = f.photo_id === lockedPhoto
                        const owner = f.person_id === null ? t('face.unassigned') : (f.person_name ?? `${t('person.unnamed')} ${f.person_id}`)
                        const pct = similarityPct(f.similarity)
                        return (
                          <li key={f.face_id}>
                            <button
                              type="button"
                              className="relative block w-full overflow-hidden rounded-control border-2 transition-colors disabled:cursor-not-allowed disabled:opacity-40"
                              style={{ borderColor: on ? 'var(--accent)' : 'var(--line)' }}
                              aria-pressed={on}
                              aria-label={t('faceSearch.similarFace', { person: owner, pct })}
                              title={blocked ? t('faceSearch.samePhoto') : t('faceSearch.similarFace', { person: owner, pct })}
                              disabled={blocked || creating}
                              onClick={() => dispatch({ type: 'toggleChosen', faceId: f.face_id })}
                              data-testid="similar-face"
                            >
                              <img src={faceCropUrl(f.face_id, 128)} alt="" className="block aspect-square w-full object-cover" loading="lazy" draggable={false} />
                              {on && (
                                <span className="absolute top-0.5 left-0.5 flex h-4 w-4 items-center justify-center rounded bg-accent text-accent-fg">
                                  <Check size={11} strokeWidth={3} />
                                </span>
                              )}
                              <span className="tnum absolute right-0.5 bottom-0.5 rounded bg-black/65 px-1 text-[10px] leading-4 text-white">{pct}%</span>
                            </button>
                          </li>
                        )
                      })}
                    </ul>
                  )}
                </div>

                {naming === null ? (
                  <div className="flex items-center justify-end gap-2 pt-1 text-xs text-muted">
                    {newIds.length === 0 && <span data-testid="face-search-new-none">{t('faceSearch.newPersonNone')}</span>}
                    <button className="btn" disabled={newIds.length === 0} onClick={() => setNaming('')} data-testid="face-search-new">
                      <UserPlus size={13} />
                      {t('faceSearch.newPerson')}
                      {newIds.length > 0 && <span className="tnum rounded bg-bg px-1 text-[11px]">{newIds.length}</span>}
                    </button>
                  </div>
                ) : (
                  <form
                    className="flex flex-wrap items-center justify-end gap-2 pt-1"
                    onSubmit={(e) => {
                      e.preventDefault()
                      void createPerson()
                    }}
                    data-testid="face-search-new-form"
                  >
                    <input
                      autoFocus
                      className="field min-w-0 flex-1"
                      value={naming}
                      placeholder={t('faceSearch.namePlaceholder')}
                      aria-label={t('faceSearch.nameLabel')}
                      disabled={creating}
                      onChange={(e) => setNaming(e.target.value)}
                      data-testid="face-search-new-name"
                    />
                    <button type="button" className="btn" disabled={creating} onClick={() => setNaming(null)}>
                      {t('common.cancel')}
                    </button>
                    <button type="submit" className="btn btn-primary" disabled={creating || newIds.length === 0} data-testid="face-search-new-confirm">
                      {creating ? <Loader2 size={13} className="animate-spin" /> : <UserPlus size={13} />}
                      {t('faceSearch.create', { n: newIds.length })}
                    </button>
                  </form>
                )}
              </div>
            )}
          </div>
        </div>
      )}
      <input
        ref={input}
        type="file"
        accept="image/*"
        className="sr-only"
        aria-label={t('faceSearch.choose')}
        onChange={(e) => {
          const f = firstImage(e.target.files)
          e.target.value = ''
          if (f) go({ type: 'pick', file: f })
        }}
        data-testid="face-search-input"
      />
      {state.status === 'idle' && (
        <div className="flex justify-center">
          <button className="btn" onClick={() => input.current?.click()}>
            <ScanSearch size={14} />
            {t('faceSearch.choose')}
          </button>
        </div>
      )}
    </div>
  )
}
