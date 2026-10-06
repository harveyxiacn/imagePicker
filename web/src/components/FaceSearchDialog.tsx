import { ImagePlus, Loader2, ScanSearch, UserPlus } from 'lucide-react'
import { useEffect, useMemo, useReducer, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useMatch, useNavigate } from 'react-router-dom'
import { api, ApiError, faceCropUrl } from '@/api/client'
import { usePeople } from '@/api/queries'
import { faceSearchReducer, firstImage, initialFaceSearch, similarityPct } from '@/lib/faceSearch'
import { DEFAULT_PERSON_FILTER } from '@/lib/filter'
import { useToasts } from '@/stores/toasts'
import { useUi } from '@/stores/ui'
import { Modal } from './Modal'

/**
 * Search by face (F2.13.4): drop / pick / paste a photo, choose a detected face, see the matching people with
 * similarity; clicking one filters the library by that person.
 */
export function FaceSearchDialog({ sessionId }: { sessionId: number }) {
  const { t } = useTranslation()
  const open = useUi((s) => s.faceSearchOpen)
  const setOpen = useUi((s) => s.setFaceSearchOpen)
  const [state, dispatch] = useReducer(faceSearchReducer, initialFaceSearch)
  const [over, setOver] = useState(false)
  const input = useRef<HTMLInputElement>(null)
  const people = usePeople(sessionId)
  const navigate = useNavigate()
  const inLibrary = useMatch('/s/:sessionId')

  // Run the request whenever the machine enters `searching`.
  useEffect(() => {
    if (state.status !== 'searching') return
    let alive = true
    api
      .faceSearch({ image: state.file }, { session_id: sessionId, ...(state.faceIndex !== null ? { face_index: state.faceIndex } : {}) })
      .then((resp) => alive && dispatch({ type: 'result', resp }))
      .catch((err: unknown) => alive && dispatch({ type: 'fail', message: err instanceof ApiError || err instanceof Error ? err.message : String(err) }))
    return () => {
      alive = false
    }
  }, [state, sessionId])

  // Paste an image from the clipboard while the dialog is open.
  useEffect(() => {
    if (!open) return
    const onPaste = (e: ClipboardEvent) => {
      const file = firstImage(e.clipboardData?.files)
      if (file) {
        e.preventDefault()
        dispatch({ type: 'pick', file })
      }
    }
    document.addEventListener('paste', onPaste)
    return () => document.removeEventListener('paste', onPaste)
  }, [open])

  const file = state.status === 'idle' ? null : state.file
  const previewUrl = useMemo(() => (file ? URL.createObjectURL(file) : null), [file])
  useEffect(() => () => void (previewUrl && URL.revokeObjectURL(previewUrl)), [previewUrl])

  const faces = state.status === 'results' ? state.resp.faces_detected : state.status === 'searching' ? state.faces : []
  const selected = state.status === 'results' ? state.faceIndex : state.status === 'searching' ? state.faceIndex : null
  const coverOf = (personId: number) => people.data?.find((p) => p.id === personId)?.cover_face_id

  const close = (o: boolean) => {
    setOpen(o)
    if (!o) dispatch({ type: 'reset' })
  }

  const pickPerson = (personId: number) => {
    const person = { ...DEFAULT_PERSON_FILTER, include: [personId] }
    if (inLibrary) useUi.getState().setFilter({ person })
    else {
      useUi.getState().openWithFilter({ person })
      navigate(`/s/${sessionId}`)
    }
    close(false)
  }

  return (
    <Modal open={open} onOpenChange={close} title={t('faceSearch.title')} description={t('faceSearch.desc')} width="max-w-2xl">
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
              if (f) dispatch({ type: 'pick', file: f })
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
                    onClick={() => dispatch({ type: 'selectFace', index: i })}
                    data-testid="detected-face"
                  >
                    <span className="absolute -top-px -left-px rounded-br bg-black/70 px-1 text-[10px] leading-4 text-white">{i + 1}</span>
                  </button>
                ))}
              </div>
              <button className="btn btn-ghost" onClick={() => dispatch({ type: 'reset' })} data-testid="face-search-again">
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
              {state.status === 'results' && (
                <div className="flex flex-col gap-2" data-testid="face-search-results">
                  <div className="text-xs text-muted">{t('faceSearch.candidates')}</div>
                  {state.resp.candidates.length === 0 ? (
                    <div className="py-4 text-center text-muted">{t('faceSearch.noMatch')}</div>
                  ) : (
                    <ul className="flex flex-col gap-1.5">
                      {state.resp.candidates.map((c) => {
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
                  <div className="flex items-center justify-between pt-1 text-xs text-muted">
                    <span>{t('faceSearch.similarCount', { n: state.resp.similar_faces.length })}</span>
                    <button className="btn" onClick={() => useToasts.getState().push('info', t('common.soon'), 2000)} data-testid="face-search-new">
                      <UserPlus size={13} />
                      {t('faceSearch.newPerson')}
                    </button>
                  </div>
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
            if (f) dispatch({ type: 'pick', file: f })
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
    </Modal>
  )
}
