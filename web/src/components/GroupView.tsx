import { ArrowLeftRight, Ban, ChevronLeft, ChevronRight, Flag as FlagIcon, Link2, Link2Off, Scissors, Sparkles, Merge, ThumbsUp } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { Photo } from '@/api/types'
import { issueBadges } from '@/lib/ai'
import { useKeyHeld } from '@/lib/hooks'
import { FIT_VIEW, type ViewState } from '@/lib/zoom'
import type { GroupApi } from '@/pages/useGroup'
import { useUi } from '@/stores/ui'
import { ExpressionMatrix } from './ExpressionMatrix'
import { FaceBoxes } from './FaceBoxes'
import { Filmstrip } from './Filmstrip'
import { ZoomPane } from './ZoomPane'
import { AiStars, StarRating } from './controls'

interface Props {
  group: GroupApi
  /** The two compared photos (A pinned, B active). */
  slots: Photo[]
  onPick: (id: number, e?: React.MouseEvent) => void
  onSwap: () => void
  onRate: (id: number, n: number | null) => void
}

const LABELS = ['A', 'B']

/** Group view (doc 04 3.3): compare panes + filmstrip + person x frame expression matrix + group actions. */
export function GroupView({ group, slots, onPick, onSwap, onRate }: Props) {
  const { t } = useTranslation()
  const sync = useUi((s) => s.syncZoom)
  const setSync = useUi((s) => s.setSyncZoom)
  const showFaces = useUi((s) => s.showFaces)
  const toggleSignal = useUi((s) => s.zoomToggle)
  const activeId = useUi((s) => s.activeId)
  const compareA = useUi((s) => s.compareA)
  const [shared, setShared] = useState<ViewState>(FIT_VIEW)
  const [per, setPer] = useState<Record<number, ViewState>>({})
  const [hover, setHover] = useState({ nx: 0.5, ny: 0.5 })
  const zHeld = useKeyHeld('z')

  const { burst, index, count, photos } = group
  const sceneLabel = photos[0]?.scene_type ? t(`scene_type.${photos[0].scene_type}`) : null

  // Plain click on a frame = new B candidate; Shift+click pins it as A.
  const pick = (id: number, e?: React.MouseEvent) => {
    if (e?.shiftKey) {
      useUi.getState().setCompareA(id)
      if (id === useUi.getState().activeId) {
        const other = photos.find((p) => p.id !== id)
        if (other) useUi.getState().setActive(other.id)
      }
      return
    }
    onPick(id, e)
  }

  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="group-view">
      <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-line px-3 py-1.5">
        <div className="mr-1 flex items-center gap-2">
          <span className="font-medium" data-testid="group-title">
            {t('group.title', { i: index >= 0 ? index + 1 : '–', n: count })}
          </span>
          <span className="text-muted">
            {t('group.size', { n: burst?.size ?? photos.length })}
            {sceneLabel && ` · ${sceneLabel}`}
          </span>
        </div>
        <button className="btn" onClick={() => group.go(-1)} disabled={index <= 0} title={`${t('group.prev')} (,)`} data-testid="group-prev">
          <ChevronLeft size={14} />
          {t('group.prev')}
        </button>
        <button className="btn" onClick={() => group.go(1)} disabled={index < 0 || index >= count - 1} title={`${t('group.next')} (.)`} data-testid="group-next">
          {t('group.next')}
          <ChevronRight size={14} />
        </button>
        <button className="btn" aria-pressed={sync} onClick={() => setSync(!sync)} title={t('compare.syncHint')}>
          {sync ? <Link2 size={14} /> : <Link2Off size={14} />}
          {t('compare.sync')}
        </button>
        <button className="btn" onClick={onSwap} disabled={compareA === null} title={`${t('keys.swap')} (Tab)`}>
          <ArrowLeftRight size={14} />
          {t('compare.swap')}
        </button>
        <div className="ml-auto flex flex-wrap items-center gap-2">
          <button className="btn" onClick={() => void group.split()} title={t('group.splitHint')} data-testid="group-split">
            <Scissors size={14} />
            {t('group.split')}
          </button>
          <button className="btn" onClick={() => void group.merge(-1)} title={t('group.mergePrev')}>
            <Merge size={14} />
            {t('group.mergePrevShort')}
          </button>
          <button className="btn" onClick={() => void group.merge(1)} title={t('group.mergeNext')}>
            <Merge size={14} />
            {t('group.mergeNextShort')}
          </button>
          <button className="btn" onClick={() => group.pickA(false)} title={`${t('group.pickA')} (Enter)`} data-testid="group-pick">
            <ThumbsUp size={14} />
            {t('group.pickA')}
          </button>
          <button className="btn btn-primary" onClick={() => void group.keepBest()} title={t('group.keepBestHint')} data-testid="group-keep-best">
            <Ban size={14} />
            {t('group.keepBest')}
          </button>
        </div>
      </div>

      <div className="grid min-h-[200px] flex-1 gap-1 bg-line p-1" style={{ gridTemplateColumns: `repeat(${Math.max(1, slots.length)}, minmax(0, 1fr))` }}>
        {group.loading && slots.length === 0 && <div className="flex items-center justify-center bg-bg text-muted">{t('library.loading')}</div>}
        {slots.map((p, i) => {
          const isA = p.id === compareA
          const isActive = p.id === activeId
          const view = sync ? shared : (per[i] ?? FIT_VIEW)
          const badges = issueBadges(p.issues)
          return (
            <div
              key={p.id}
              className="relative min-h-0 bg-bg"
              style={{ outline: isActive ? '2px solid var(--accent)' : 'none', outlineOffset: -2 }}
              data-testid={`group-pane-${LABELS[i]}`}
            >
              <ZoomPane
                photo={p}
                view={view}
                toggleSignal={toggleSignal}
                onViewChange={(v) => (sync ? setShared(v) : setPer((s) => ({ ...s, [i]: v })))}
                hold={zHeld ? hover : null}
                onHover={(nx, ny) => setHover({ nx, ny })}
                overlay={showFaces && p.analyzed ? <FaceBoxes photoId={p.id} /> : undefined}
              />
              <div className="pointer-events-none absolute top-2 left-2 flex items-center gap-2 rounded bg-black/60 px-2 py-1 text-white backdrop-blur">
                <span className={`rounded px-1.5 text-xs font-bold ${isA ? 'bg-accent text-accent-fg' : 'bg-white/20'}`}>{LABELS[i]}</span>
                <span className="tnum text-xs">{p.file_name}</span>
                {p.rank_in_burst === 0 && (
                  <span className="flex items-center gap-0.5 rounded bg-ai px-1 text-[11px] font-semibold text-white">
                    <Sparkles size={10} />
                    {t('group.recommended')}
                  </span>
                )}
                {p.flag === 1 && <FlagIcon size={12} className="text-success" fill="currentColor" />}
                {p.flag === -1 && <Ban size={12} className="text-danger" />}
                {badges.map((b) => (
                  <span key={b.issue} title={t(`issue.${b.label}`)}>
                    {b.glyph}
                  </span>
                ))}
              </div>
              <div className="absolute bottom-2 left-2 flex items-center gap-3 rounded bg-black/60 px-2 py-0.5 backdrop-blur">
                <StarRating value={p.user_rating} onChange={(n) => onRate(p.id, n)} size={14} />
                <AiStars value={p.ai_rating} size={12} />
              </div>
            </div>
          )
        })}
      </div>

      <Filmstrip photos={photos} activeId={activeId} markedIds={compareA !== null ? [compareA] : []} numbered onPick={pick} />

      <div className="max-h-[38%] shrink-0 overflow-y-auto border-t border-line bg-panel">
        {group.burstId !== null && (
          <ExpressionMatrix burstId={group.burstId} photos={photos} aId={compareA} bId={activeId} onPick={pick} />
        )}
      </div>
    </div>
  )
}
