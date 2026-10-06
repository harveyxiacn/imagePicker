import { AlertTriangle, Loader2, RotateCcw } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { Photo } from '@/api/types'
import type { PersonView } from '@/lib/besttake'
import { stackKey, withoutCrop } from '@/lib/edit'
import { useElementSize } from '@/lib/hooks'
import { FIT_VIEW, type ViewState } from '@/lib/zoom'
import { useEdit } from '@/stores/edit'
import { useGen } from '@/stores/gen'
import { ZoomPane } from '../ZoomPane'
import { usePreviewFrame } from '../edit/usePreviewFrame'

export interface FaceBox {
  key: number
  /** normalised [x, y, w, h] of the face in the base photo (upright, pre-crop) */
  box: [number, number, number, number]
}

interface Props {
  photo: Photo
  people: PersonView[]
  boxes: FaceBox[]
  selected: number | null
  onSelect: (key: number) => void
  /** show the untouched original instead of the composite */
  original: boolean
}

/** Composite preview of the base photo (render preview with the base stack) with a clickable box per person. */
export function BestTakeCanvas({ photo, people, boxes, selected, onSelect, original }: Props) {
  const { t } = useTranslation()
  const stack = useEdit((s) => s.stack)
  const ready = useEdit((s) => s.loaded && s.photoId === photo.id)
  const tasks = useGen((s) => s.tasks)
  const results = useGen((s) => s.results)
  const [box, size] = useElementSize<HTMLDivElement>()
  const [viewState, setViewState] = useState<{ key: number; view: ViewState }>({ key: -1, view: FIT_VIEW })
  const view = viewState.key === photo.id ? viewState.view : FIT_VIEW

  const main = useMemo(() => {
    if (original) return { body: { original: true as const }, key: 'orig' }
    const s = withoutCrop(stack)
    return { body: { stack: s }, key: `bt|${stackKey(s)}` }
  }, [stack, original])
  const frame = usePreviewFrame({ photoId: photo.id, body: main.body, bodyKey: main.key, dragging: false, container: size, enabled: ready }).frame

  const running = Object.values(tasks).filter((x) => x.kind === 'besttake' && x.state === 'running')

  const overlay = (
    <>
      {boxes.map((b) => {
        const person = people.find((p) => p.key === b.key)
        if (!person) return null
        const task = running.find((x) => x.baseFaceIds.includes(person.baseFaceId) || x.baseFaceIds.length === 0)
        const result = results[person.baseFaceId]
        const warn = result?.ok && result.warnings.length > 0
        const failed = result && !result.ok
        const isSel = selected === b.key
        const pct = task && task.total > 0 ? Math.round((task.done / task.total) * 100) : 0
        const pad = 0.01
        return (
          <button
            key={b.key}
            type="button"
            className="pointer-events-auto absolute rounded-md transition-colors"
            style={{
              left: `${(b.box[0] - pad) * 100}%`,
              top: `${(b.box[1] - pad) * 100}%`,
              width: `${(b.box[2] + pad * 2) * 100}%`,
              height: `${(b.box[3] + pad * 2) * 100}%`,
              border: `2px ${isSel ? 'solid' : 'dashed'} ${failed ? 'var(--danger)' : isSel ? 'var(--accent)' : 'rgba(255,255,255,0.75)'}`,
              boxShadow: isSel ? '0 0 0 2px rgba(0,0,0,.45), 0 0 18px color-mix(in srgb, var(--accent) 60%, transparent)' : '0 0 0 1px rgba(0,0,0,.4)',
              background: isSel ? 'color-mix(in srgb, var(--accent) 10%, transparent)' : undefined,
            }}
            aria-pressed={isSel}
            aria-label={person.name ?? `#${person.person_id ?? person.track_id}`}
            onPointerDown={(e) => e.stopPropagation()}
            onDoubleClick={(e) => e.stopPropagation()}
            onClick={() => onSelect(b.key)}
            data-testid="bt-face"
            data-selected={isSel}
            data-replaced={person.replaced}
          >
            <span className="absolute -top-5 left-0 flex max-w-full items-center gap-1 truncate rounded bg-black/70 px-1 text-[10px] leading-4 text-white">
              {person.name ?? t('person.unnamed')}
              {person.replaced && (
                <span className="text-ai" title={t('besttake.replaced')} data-testid="bt-replaced-icon">
                  <RotateCcw size={10} />
                </span>
              )}
              {warn && (
                <span className="text-warning" title={t('besttake.hasWarnings')}>
                  <AlertTriangle size={10} />
                </span>
              )}
            </span>
            {task && (
              <span className="absolute inset-0 flex flex-col items-center justify-center gap-1 rounded-md bg-black/55 text-white" data-testid="bt-face-progress">
                <Loader2 size={18} className="animate-spin" />
                <span className="tnum text-[11px]">{pct}%</span>
              </span>
            )}
          </button>
        )
      })}
    </>
  )

  return (
    <div ref={box} className="relative h-full w-full" data-testid="bt-canvas">
      <ZoomPane photo={photo} view={view} onViewChange={(v) => setViewState({ key: photo.id, view: v })} src={frame?.url ?? null} imageSize={frame ? { w: frame.w, h: frame.h } : undefined} overlay={overlay} />
      {original && <span className="pointer-events-none absolute top-2 left-2 rounded bg-black/65 px-1.5 py-0.5 text-xs font-medium text-white">{t('edit.original')}</span>}
    </div>
  )
}
