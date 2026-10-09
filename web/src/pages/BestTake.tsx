import { useQueryClient } from '@tanstack/react-query'
import { Check, ChevronDown, ChevronLeft, Eclipse, Loader2, Redo2, Sparkles, Undo2 } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { api, thumbUrl } from '@/api/client'
import { useBestTakePlan, useBurstFaces, useBurstPhotos, usePhotoAnalysis } from '@/api/queries'
import { BestTakePanel } from '@/components/besttake/BestTakePanel'
import { BestTakeCanvas, type FaceBox } from '@/components/besttake/BestTakeCanvas'
import { GenTasks } from '@/components/GenTasks'
import { HelpOverlay } from '@/components/HelpOverlay'
import { ModelConsentDialog } from '@/components/ModelConsentDialog'
import { qk } from '@/lib/cache'
import { redo, undo } from '@/lib/actions'
import { alreadyBest, autoChoices, baseWhy, buildPlanView, choiceFor, frameNumber, frameWork, type CandidateView, type PersonView } from '@/lib/besttake'
import { changeStack, flushSaves } from '@/lib/editActions'
import { runGen } from '@/lib/gen'
import { useHistory } from '@/lib/history'
import { dispatchKey, isTextEntryTarget, type Handlers } from '@/lib/keymap'
import { removeBestTake } from '@/lib/patches'
import { useEdit } from '@/stores/edit'
import { useGen } from '@/stores/gen'
import { useToasts } from '@/stores/toasts'
import { useUi } from '@/stores/ui'

const timeOrder = <T extends { taken_at: number | null; id: number }>(a: T, b: T) => (a.taken_at ?? 0) - (b.taken_at ?? 0) || a.id - b.id

/** Best Take editor (doc 04 3.4). Route `/s/:sessionId/besttake/:burstId[?base=photoId]`. */
export function BestTake() {
  const { sessionId: rawS, burstId: rawB } = useParams()
  const sessionId = Number(rawS)
  const burstId = Number(rawB)
  const [search] = useSearchParams()
  const { t } = useTranslation()
  const qc = useQueryClient()
  const navigate = useNavigate()

  const planQ = useBestTakePlan(burstId)
  const plan = planQ.data
  const photosQ = useBurstPhotos(sessionId, burstId)
  const photos = useMemo(() => [...(photosQ.data?.photos ?? [])].sort(timeOrder), [photosQ.data])
  const order = useMemo(() => photos.map((p) => p.id), [photos])
  const facesQ = useBurstFaces(burstId)

  // An explicit choice carries a timestamp: a later `besttake/auto` result (it may pick another base photo) wins over it.
  const [choice, setChoice] = useState<{ id: number | null; at: number }>(() => ({ id: Number(search.get('base')) || null, at: Date.now() }))
  const [baseMenu, setBaseMenu] = useState(false)
  const menuOpen = useRef(false)
  useEffect(() => {
    menuOpen.current = baseMenu
  })
  const [selected, setSelected] = useState<number | null>(null)
  const [original, setOriginal] = useState(false)
  const autoBase = useGen((s) => s.autoBase)
  const chosenBase = autoBase && autoBase.nonce > choice.at ? autoBase.photoId : choice.id
  const setChosenBase = (id: number | null) => setChoice({ id, at: Date.now() })
  const baseId = chosenBase ?? plan?.base_photo_id ?? null
  const basePhoto = photos.find((p) => p.id === baseId)

  // Load the saved stack of the base photo into the shared edit store (history / live preview use it).
  useEffect(() => {
    if (baseId === null) return
    let alive = true
    void flushSaves(qc)
      .then(() => qc.fetchQuery({ queryKey: qk.edits(baseId), queryFn: () => api.edits(baseId), staleTime: 0 }))
      .then((res) => {
        if (alive) useEdit.getState().load(baseId, res.stack)
      })
      .catch((err: unknown) => useToasts.getState().push('error', err instanceof Error ? err.message : String(err), 5000))
    useUi.getState().setActive(baseId)
    return () => {
      alive = false
    }
  }, [baseId, qc])
  useEffect(
    () => () => {
      void flushSaves(qc)
      useEdit.setState({ photoId: null, loaded: false, dragging: false, compare: 'off' })
    },
    [qc],
  )

  const stack = useEdit((s) => s.stack)
  const analysis = usePhotoAnalysis(baseId ?? undefined).data
  const faceIdOf = useCallback((trackId: number, photoId: number) => facesQ.data?.tracks.find((tr) => tr.track_id === trackId)?.cells[String(photoId)]?.id, [facesQ.data])
  const view = useMemo(() => (plan ? buildPlanView(plan, { baseId: baseId ?? undefined, order, stack, faceIdOf }) : []), [plan, baseId, order, stack, faceIdOf])
  const boxes: FaceBox[] = useMemo(
    () =>
      view.flatMap((p) => {
        const f = analysis?.faces.find((x) => x.id === p.baseFaceId)
        return f ? [{ key: p.key, box: f.bbox }] : []
      }),
    [view, analysis],
  )

  // default selection: the first person who can still be improved
  const selectedKey = selected !== null && view.some((p) => p.key === selected) ? selected : (view.find((p) => p.bestSwap)?.key ?? view[0]?.key ?? null)
  const person = view.find((p) => p.key === selectedKey)
  const tasks = useGen((s) => s.tasks)
  const busy = Object.values(tasks).some((x) => x.kind === 'besttake' && x.state === 'running')

  const back = useCallback(() => {
    void flushSaves(qc)
    navigate(`/s/${sessionId}`, { state: { keep: true } })
  }, [qc, navigate, sessionId])

  const who = (p: PersonView) => p.name ?? t('person.unnamed')

  const choose = (p: PersonView, c: CandidateView) => {
    if (baseId === null) return
    if (c.isBase) {
      // the base's own face = keep the original
      if (p.replaced) restore(p)
      return
    }
    if (!c.composable) return
    useGen.getState().clearResult(p.baseFaceId)
    void runGen(qc, {
      kind: 'besttake',
      sessionId,
      photoId: baseId,
      label: t('history.bestTake', { who: who(p) }),
      baseFaceIds: [p.baseFaceId],
      start: () => api.bestTake(baseId, [choiceFor(p, c)]),
    })
  }

  function restore(p: PersonView) {
    if (p.person_id === null) return
    useGen.getState().clearResult(p.baseFaceId)
    changeStack(qc, removeBestTake(useEdit.getState().stack, p.person_id), t('history.bestTakeRestore', { who: who(p) }))
  }

  const autoAll = () => {
    if (baseId === null) return
    void runGen(qc, {
      kind: 'besttake',
      sessionId,
      photoId: baseId,
      label: t('history.bestTakeAuto'),
      snapshotIds: order,
      start: () => api.bestTakeAuto(burstId),
    })
  }
  /** Client-side variant on the chosen base: one POST with every person's best candidate. */
  const autoOnBase = () => {
    const choices = autoChoices(view)
    if (baseId === null || choices.length === 0) return
    void runGen(qc, {
      kind: 'besttake',
      sessionId,
      photoId: baseId,
      label: t('history.bestTakeAuto'),
      baseFaceIds: choices.map((c) => c.base_face_id),
      start: () => api.bestTake(baseId, choices),
    })
  }

  const canUndo = useHistory((s) => s.undoStack.length > 0)
  const canRedo = useHistory((s) => s.redoStack.length > 0)
  const doUndo = useCallback(async () => {
    const e = await undo(qc)
    useToasts.getState().push('info', e ? t('history.undone', { label: e.label }) : t('history.nothingToUndo'), 1800)
  }, [qc, t])
  const doRedo = useCallback(async () => {
    const e = await redo(qc)
    useToasts.getState().push('info', e ? t('history.redone', { label: e.label }) : t('history.nothingToRedo'), 1800)
  }, [qc, t])

  useEffect(() => {
    const handlers: Handlers = {
      'edit.undo': () => void doUndo(),
      'edit.redo': () => void doRedo(),
      'edit.before': () => setOriginal((v) => !v),
      'edit.close': () => (menuOpen.current ? setBaseMenu(false) : back()),
      'help.toggle': () => useUi.getState().setHelpOpen(!useUi.getState().helpOpen),
    }
    const onKey = (e: KeyboardEvent) => {
      if (isTextEntryTarget(e.target)) return
      if (document.querySelector('[role="dialog"]') && e.key !== '?') return
      if (e.repeat && e.key.toLowerCase() !== 'z' && e.key.toLowerCase() !== 'y') return
      if (dispatchKey(e, 'edit', handlers)) e.preventDefault()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [doUndo, doRedo, back])

  if (planQ.isError) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3" data-testid="bt-error">
        <div className="text-danger">{(planQ.error as Error).message}</div>
        <Link to={`/s/${sessionId}`} className="btn">
          {t('common.back')}
        </Link>
      </div>
    )
  }

  const improvable = view.filter((p) => p.bestSwap).length
  const baseNum = baseId !== null ? frameNumber(order, baseId) : null
  const isAutoBase = plan !== undefined && baseId === plan.base_photo_id && chosenBase === null
  const why = plan && isAutoBase ? baseWhy(plan, order) : undefined
  const optionTitle = (p: { id: number; file_name: string }) => {
    const n = frameWork(plan, p.id)
    return n === undefined ? p.file_name : `${p.file_name} · ${t('besttake.frameWork', { n })}`
  }
  const done = alreadyBest(view).length === view.length && view.length > 0

  return (
    <div className="flex h-full flex-col" data-testid="besttake-page">
      <header className="relative z-20 flex h-12 shrink-0 items-center gap-2 border-b border-line px-2 sm:gap-3 sm:px-3">
        <button className="btn btn-ghost" onClick={back} aria-label={t('common.back')} data-testid="bt-back">
          <ChevronLeft size={16} />
          <span className="hidden sm:inline">{t('common.back')}</span>
        </button>
        <div className="flex min-w-0 items-center gap-2 font-semibold">
          <Sparkles size={15} className="shrink-0 text-ai" />
          <span className="truncate" data-testid="bt-title">
            {t('besttake.title')}
          </span>
          <span className="relative">
            <button className="btn" onClick={() => setBaseMenu((v) => !v)} aria-expanded={baseMenu} title={why ? t(why.key, why.params) : undefined} data-testid="bt-base-button">
              {t('besttake.base')} #{baseNum ?? '?'}
              <span className="text-xs font-normal text-muted">{isAutoBase ? t('besttake.autoPicked') : ''}</span>
              <ChevronDown size={13} />
            </button>
            {baseMenu && (
              <div className="absolute top-full left-0 z-30 mt-1 flex w-max max-w-[80vw] gap-1.5 overflow-x-auto rounded-card border border-line bg-elevated p-2 shadow-[var(--shadow)]" data-testid="bt-base-menu">
                {photos.map((p, i) => (
                  <button
                    key={p.id}
                    className="relative shrink-0 overflow-hidden rounded-control border"
                    style={{ borderColor: p.id === baseId ? 'var(--accent)' : 'var(--line)', boxShadow: p.id === baseId ? '0 0 0 2px var(--accent)' : undefined }}
                    onClick={() => {
                      setChosenBase(p.id)
                      setBaseMenu(false)
                      setSelected(null)
                    }}
                    title={optionTitle(p)}
                    data-testid="bt-base-option"
                  >
                    <img src={thumbUrl(p, 256)} alt="" className="h-14 w-20 object-cover" draggable={false} />
                    <span className="tnum absolute bottom-0 left-0 rounded-tr bg-black/70 px-1 text-[10px] text-white">#{i + 1}</span>
                    {p.id === plan?.base_photo_id && <span className="absolute top-0 right-0 rounded-bl bg-ai px-1 text-[10px] text-white">★</span>}
                  </button>
                ))}
              </div>
            )}
          </span>
        </div>
        <div className="flex items-center">
          <button className="btn btn-ghost btn-icon" disabled={!canUndo} onClick={() => void doUndo()} aria-label={t('keys.undo')} title={`${t('keys.undo')} (Ctrl+Z)`} data-testid="bt-undo">
            <Undo2 size={15} />
          </button>
          <button className="btn btn-ghost btn-icon" disabled={!canRedo} onClick={() => void doRedo()} aria-label={t('keys.redo')} title={`${t('keys.redo')} (Ctrl+Shift+Z)`} data-testid="bt-redo">
            <Redo2 size={15} />
          </button>
        </div>
        <div className="ml-auto flex items-center gap-2">
          <button className="btn btn-primary" disabled={busy || view.length === 0} onClick={autoAll} title={t('besttake.autoHint')} data-testid="bt-auto">
            {busy ? <Loader2 size={14} className="animate-spin" /> : <Sparkles size={14} />}
            <span className="hidden sm:inline">{t('besttake.auto')}</span>
          </button>
          {baseId !== null && chosenBase !== null && chosenBase !== plan?.base_photo_id && (
            <button className="btn" disabled={busy || improvable === 0} onClick={autoOnBase} title={t('besttake.autoOnBaseHint')} data-testid="bt-auto-base">
              {t('besttake.autoOnBase')}
            </button>
          )}
          <button className="btn" aria-pressed={original} onClick={() => setOriginal((v) => !v)} title={`${t('edit.toggleOriginal')} (\\)`} data-testid="bt-compare">
            <Eclipse size={14} />
            <span className="hidden md:inline">{t('besttake.compare')}</span>
            <kbd>\</kbd>
          </button>
          <button className="btn" onClick={back} data-testid="bt-done">
            <Check size={14} />
            {t('besttake.finish')}
          </button>
        </div>
      </header>

      <div className="relative flex min-h-0 flex-1">
        <main className="min-w-0 flex-1">
          {basePhoto && plan ? (
            <BestTakeCanvas photo={basePhoto} people={view} boxes={boxes} selected={selectedKey} onSelect={setSelected} original={original} />
          ) : (
            <div className="flex h-full items-center justify-center text-muted">
              <Loader2 className="animate-spin" size={18} />
            </div>
          )}
        </main>
        <aside className="w-[320px] shrink-0 overflow-y-auto border-l border-line bg-panel" aria-label={t('besttake.panel')}>
          {plan && view.length === 0 ? (
            <div className="p-6 text-center text-muted" data-testid="bt-nobody">
              {t('besttake.noPeople')}
            </div>
          ) : (
            <BestTakePanel person={person} retargeted={chosenBase !== null && chosenBase !== plan?.base_photo_id} busy={busy} onChoose={choose} onRestore={restore} />
          )}
        </aside>
      </div>

      <div className="flex h-9 shrink-0 items-center gap-3 border-t border-line bg-panel px-3 text-xs text-muted" data-testid="bt-footer">
        <span>{done ? t('besttake.allBest') : t('besttake.summary', { n: improvable, total: view.length })}</span>
        <span className="ml-auto">
          <GenTasks />
        </span>
      </div>
      <HelpOverlay />
      <ModelConsentDialog />
    </div>
  )
}
