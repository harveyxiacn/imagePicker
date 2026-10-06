import { useQueryClient } from '@tanstack/react-query'
import { Check, ChevronLeft, SkipForward, Star, Undo2, X } from 'lucide-react'
import { useEffect, useMemo, useReducer, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { previewUrl, thumbUrl } from '@/api/client'
import type { Flag, Photo } from '@/api/types'
import { editPhotos, undo } from '@/lib/actions'
import { haptic } from '@/lib/haptics'
import { liveDirection, swipeProgress, type SwipeDir } from '@/lib/gestures'
import { useElementSize } from '@/lib/hooks'
import {
  buildDeck,
  currentCard,
  deckProgress,
  deckReducer,
  decisionFor,
  initDeck,
  isDone,
  stackBehind,
  type Decision,
} from '@/lib/quickCull'
import { AiStars } from '../controls'
import { useGestureSurface } from './useGestureSurface'

interface Props {
  photos: Photo[]
  onBack: () => void
}

const EXIT_MS = 200

/** Quick cull (快速挑片): per group only the AI top-3 as a stack of cards; right = keep, left = reject, up = skip group. */
export function QuickCull({ photos, onBack }: Props) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  // The deck is built once per mount: acting on cards changes photo data, which must not reshuffle it.
  const [deck, dispatch] = useReducer(deckReducer, photos, (p) => initDeck(buildDeck(p)))
  const byId = useMemo(() => new Map(photos.map((p) => [p.id, p])), [photos])
  const [boxRef, box] = useElementSize<HTMLDivElement>()
  const [exit, setExit] = useState<SwipeDir | null>(null)
  const cardId = currentCard(deck)
  const card = cardId === null ? undefined : byId.get(cardId)
  const prog = deckProgress(deck)
  const done = isDone(deck)

  const latest = useRef({ cardId, card })
  useEffect(() => {
    latest.current = { cardId, card }
  })

  const decide = (dir: 'left' | 'right') => {
    const { cardId: id } = latest.current
    if (id === null) return
    const decision: Decision = decisionFor(dir)
    haptic(decision === 'keep' ? 'pick' : 'reject')
    const flag: Flag = decision === 'keep' ? 1 : -1
    void editPhotos(qc, [id], { flag }, t('history.flag'))
    dispatch({ type: 'decide', decision })
  }

  const swipe = (dir: SwipeDir) => {
    if (dir === 'down') return false
    setExit(dir)
    setTimeout(() => {
      if (dir === 'up') {
        haptic('tick')
        dispatch({ type: 'skipGroup' })
      } else decide(dir)
      setExit(null)
      g.resetDrag()
    }, EXIT_MS)
    return true
  }

  const g = useGestureSurface({ box, resetKey: cardId, zoomable: false, allow: ['left', 'right', 'up'], onSwipe: swipe })

  const undoLast = () => {
    if (!deck.history.length) return
    void undo(qc)
    haptic('tick')
    dispatch({ type: 'undo' })
  }

  const kept = Object.values(deck.decisions).filter((d) => d === 'keep').length
  const rejected = Object.values(deck.decisions).length - kept

  if (deck.groups.length === 0) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 p-6 text-center" data-testid="quick-empty">
        <div className="text-muted">{t('mobile.quick.noGroups')}</div>
        <button className="btn" onClick={onBack}>
          {t('mobile.quick.back')}
        </button>
      </div>
    )
  }

  const dir = liveDirection(g.drag.dx, g.drag.dy)
  const right = swipeProgress(g.drag.dx, g.drag.dy, 'right')
  const left = swipeProgress(g.drag.dx, g.drag.dy, 'left')
  const upP = swipeProgress(g.drag.dx, g.drag.dy, 'up')
  const fly = exit === 'left' ? { x: -1.2, y: 0 } : exit === 'right' ? { x: 1.2, y: 0 } : exit === 'up' ? { x: 0, y: -1.2 } : null
  const tx = fly ? fly.x * box.w : g.drag.dx
  const ty = fly ? fly.y * box.h : g.drag.dy
  const rot = fly ? fly.x * 14 : Math.max(-14, Math.min(14, g.drag.dx / 14))
  const animating = exit !== null || (g.drag.dx === 0 && g.drag.dy === 0)
  const behind = stackBehind(deck)

  return (
    <div className="flex h-full min-h-0 flex-col bg-bg" data-testid="quick-cull">
      <header className="flex shrink-0 items-center gap-2 px-2 py-2">
        <button className="btn btn-icon btn-ghost" onClick={onBack} aria-label={t('common.back')} data-testid="quick-back">
          <ChevronLeft size={20} />
        </button>
        <div className="min-w-0 flex-1">
          <div className="tnum text-base font-semibold" data-testid="quick-group">
            {done ? t('mobile.quick.allDone') : t('mobile.quick.group', { n: prog.group, total: prog.groups })}
          </div>
          <div className="tnum text-xs text-muted" data-testid="quick-progress">
            {t('mobile.quick.progress', { done: prog.decided, total: prog.total })}
          </div>
        </div>
        <div className="flex gap-1.5" aria-hidden>
          {Array.from({ length: prog.cards }, (_, i) => (
            <span
              key={i}
              className={`h-2.5 w-2.5 rounded-full ${done || i < prog.card - 1 ? 'bg-accent' : i === prog.card - 1 ? 'bg-accent ring-2 ring-accent/40' : 'bg-line'}`}
            />
          ))}
        </div>
      </header>
      <div className="mx-3 mb-2 h-1 shrink-0 overflow-hidden rounded bg-line" aria-hidden>
        <div className="h-full bg-accent transition-[width] duration-300" style={{ width: `${prog.total ? (prog.decided / prog.total) * 100 : 0}%` }} />
      </div>

      <div ref={boxRef} className="relative min-h-0 flex-1 px-4" data-testid="quick-stack">
        {done ? (
          <div className="flex h-full flex-col items-center justify-center gap-3 text-center" data-testid="quick-done">
            <Check size={44} className="text-success" />
            <div className="text-xl font-semibold">{t('mobile.quick.allDone')}</div>
            <div className="tnum text-muted">{t('mobile.quick.summary', { kept, rejected })}</div>
            <button className="btn btn-primary" onClick={onBack}>
              {t('mobile.quick.back')}
            </button>
          </div>
        ) : (
          <>
            {/* cards behind, deepest first */}
            {[...behind].reverse().map((id, i, arr) => {
              const p = byId.get(id)
              const depth = arr.length - i // 1 = directly behind
              if (!p) return null
              return (
                <div
                  key={id}
                  className="absolute inset-x-4 top-1 bottom-1 overflow-hidden rounded-2xl border border-line bg-panel"
                  style={{ transform: `translateY(${depth * 10}px) scale(${1 - depth * 0.045})`, opacity: 1 - depth * 0.18, transition: 'transform 200ms var(--ease)' }}
                >
                  <img src={thumbUrl(p, 256)} alt="" className="h-full w-full object-cover" draggable={false} />
                </div>
              )
            })}
            {card && (
              <div
                className="absolute inset-x-4 top-1 bottom-1 touch-none overflow-hidden rounded-2xl border border-line bg-panel shadow-[var(--shadow)] select-none"
                style={{
                  transform: `translate(${tx}px, ${ty}px) rotate(${rot}deg)`,
                  transition: animating ? `transform ${EXIT_MS}ms var(--ease), opacity ${EXIT_MS}ms` : 'none',
                  opacity: fly ? 0.2 : 1,
                }}
                {...g.bind}
                data-testid="quick-card"
                data-photo-id={card.id}
              >
                <img src={thumbUrl(card, 256)} alt="" className="pointer-events-none absolute inset-0 h-full w-full object-cover" draggable={false} />
                <img src={previewUrl(card.id, 1024, card.thumb_version)} alt={card.file_name} className="pointer-events-none absolute inset-0 h-full w-full object-cover" draggable={false} />
                <div className="pointer-events-none absolute inset-x-0 bottom-0 flex items-end justify-between bg-gradient-to-t from-black/80 to-transparent p-3 text-white">
                  <div>
                    <div className="tnum text-sm font-medium">{card.file_name}</div>
                    <AiStars value={card.ai_rating} />
                  </div>
                  <span className="tnum rounded-full bg-ai/80 px-2 py-0.5 text-xs font-bold">#{prog.card}</span>
                </div>
                {dir && (
                  <>
                    <span className="pointer-events-none absolute top-5 left-4 -rotate-12 rounded-lg border-4 border-success px-3 py-1 text-2xl font-extrabold tracking-wider text-success" style={{ opacity: right }}>
                      {t('mobile.quick.keep')}
                    </span>
                    <span className="pointer-events-none absolute top-5 right-4 rotate-12 rounded-lg border-4 border-danger px-3 py-1 text-2xl font-extrabold tracking-wider text-danger" style={{ opacity: left }}>
                      {t('mobile.quick.reject')}
                    </span>
                    <span className="pointer-events-none absolute top-1/3 left-1/2 -translate-x-1/2 rounded-lg border-4 border-white px-3 py-1 text-xl font-extrabold text-white" style={{ opacity: upP }}>
                      {t('mobile.quick.skip')}
                    </span>
                  </>
                )}
                {deck.decisions[card.id] && <span className="absolute top-3 left-3 rounded bg-black/60 px-2 py-0.5 text-xs text-white">{t('mobile.quick.decided')}</span>}
              </div>
            )}
          </>
        )}
      </div>

      <div className="flex shrink-0 items-center justify-center gap-5 px-4 py-3" data-testid="quick-actions">
        <button className="btn btn-icon !h-12 !w-12 rounded-full" onClick={undoLast} disabled={!deck.history.length} aria-label={t('mobile.cull.undo')} data-testid="quick-undo">
          <Undo2 size={20} />
        </button>
        <button className="btn btn-icon !h-14 !w-14 rounded-full border-danger text-danger" disabled={done} onClick={() => swipe('left')} aria-label={t('mobile.quick.reject')} data-testid="quick-reject">
          <X size={26} />
        </button>
        <button className="btn btn-icon !h-14 !w-14 rounded-full border-success text-success" disabled={done} onClick={() => swipe('right')} aria-label={t('mobile.quick.keep')} data-testid="quick-keep">
          <Star size={24} />
        </button>
        <button className="btn btn-icon !h-12 !w-12 rounded-full" disabled={done} onClick={() => swipe('up')} aria-label={t('mobile.quick.skip')} data-testid="quick-skip">
          <SkipForward size={20} />
        </button>
      </div>
      <div className="shrink-0 pb-2 text-center text-[11px] text-faint">{t('mobile.quick.hint')}</div>
    </div>
  )
}
