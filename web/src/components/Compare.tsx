import { ArrowLeftRight, Link2, Link2Off } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { Photo } from '@/api/types'
import { useKeyHeld } from '@/lib/hooks'
import { FIT_VIEW, type ViewState } from '@/lib/zoom'
import { useUi } from '@/stores/ui'
import { Filmstrip } from './Filmstrip'
import { ZoomPane } from './ZoomPane'
import { StarRating } from './controls'
import { Ban, Flag as FlagIcon } from 'lucide-react'

interface Props {
  photos: Photo[]
  slots: Photo[]
  activeId: number | null
  compareA: number | null
  count: 2 | 4
  sync: boolean
  onSyncChange: (b: boolean) => void
  onCountChange: (n: 2 | 4) => void
  onPick: (id: number) => void
  onSwap: () => void
  onRate: (id: number, n: number | null) => void
}

const LABELS = ['A', 'B', 'C', 'D']

/** 2-up / 4-up comparison with optionally synchronized zoom & pan. */
export function Compare({ photos, slots, activeId, compareA, count, sync, onSyncChange, onCountChange, onPick, onSwap, onRate }: Props) {
  const { t } = useTranslation()
  const [shared, setShared] = useState<ViewState>(FIT_VIEW)
  const [per, setPer] = useState<Record<number, ViewState>>({})
  const [hover, setHover] = useState({ nx: 0.5, ny: 0.5 })
  const toggleSignal = useUi((s) => s.zoomToggle)
  const zHeld = useKeyHeld('z')

  if (slots.length === 0) {
    return <div className="flex h-full items-center justify-center text-muted">{t('grid.empty')}</div>
  }

  const cols = slots.length <= 1 ? 1 : 2
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 items-center gap-2 border-b border-line px-3 py-1.5">
        <button className="btn" aria-pressed={sync} onClick={() => onSyncChange(!sync)} title={t('compare.syncHint')}>
          {sync ? <Link2 size={14} /> : <Link2Off size={14} />}
          {t('compare.sync')}
        </button>
        <button className="btn" onClick={onSwap} disabled={compareA === null} title={`${t('keys.swap')} (Tab)`}>
          <ArrowLeftRight size={14} />
          {t('compare.swap')}
        </button>
        <div className="ml-auto flex items-center gap-1" role="group" aria-label={t('compare.layout')}>
          {([2, 4] as const).map((n) => (
            <button key={n} className="btn" aria-pressed={count === n} onClick={() => onCountChange(n)}>
              {n}-up
            </button>
          ))}
        </div>
      </div>
      <div
        className="grid min-h-0 flex-1 gap-1 bg-line p-1"
        style={{ gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))`, gridAutoRows: slots.length > 2 ? 'minmax(0, 1fr)' : undefined }}
      >
        {slots.map((p, i) => {
          const isA = p.id === compareA
          const isActive = p.id === activeId
          const view = sync ? shared : (per[i] ?? FIT_VIEW)
          return (
            <div
              key={p.id}
              className="relative min-h-0 bg-bg"
              style={{ outline: isActive ? '2px solid var(--accent)' : 'none', outlineOffset: -2 }}
              onPointerDown={() => !isA && onPick(p.id)}
            >
              <ZoomPane
                photo={p}
                view={view}
                toggleSignal={toggleSignal}
                onViewChange={(v) => (sync ? setShared(v) : setPer((s) => ({ ...s, [i]: v })))}
                hold={zHeld ? hover : null}
                onHover={(nx, ny) => setHover({ nx, ny })}
              />
              <div className="pointer-events-none absolute top-2 left-2 flex items-center gap-2 rounded bg-black/60 px-2 py-1 text-white backdrop-blur">
                <span className={`rounded px-1.5 text-xs font-bold ${isActive ? 'bg-accent text-accent-fg' : 'bg-white/20'}`}>{LABELS[i]}</span>
                <span className="tnum text-xs">{p.file_name}</span>
                {p.flag === 1 && <FlagIcon size={12} className="text-success" fill="currentColor" />}
                {p.flag === -1 && <Ban size={12} className="text-danger" />}
              </div>
              <div className="absolute bottom-2 left-2 rounded bg-black/60 px-1 backdrop-blur">
                <StarRating value={p.user_rating} onChange={(n) => onRate(p.id, n)} size={14} />
              </div>
            </div>
          )
        })}
      </div>
      <Filmstrip photos={photos} activeId={activeId} markedIds={slots.map((s) => s.id)} onPick={onPick} />
    </div>
  )
}
