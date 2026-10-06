import { useQueryClient } from '@tanstack/react-query'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { Photo } from '@/api/types'
import { aspectRatio, beforeRequest, cropToolStack, FULL_RECT, getCrop, localOps, setCrop, stackKey } from '@/lib/edit'
import { commitLive, setLive } from '@/lib/editActions'
import { useElementSize } from '@/lib/hooks'
import { FIT_VIEW, type ViewState } from '@/lib/zoom'
import { useEdit } from '@/stores/edit'
import { useRepair } from '@/stores/repair'
import { ZoomPane } from '../ZoomPane'
import { CropOverlay, MaskHandles, MaskOverlay, SplitOverlay } from './Overlays'
import { BrushOverlay, BystanderOverlay } from './RepairOverlays'
import { usePreviewFrame } from './usePreviewFrame'

const SHOW_BADGE = __MOCK__ || import.meta.env.DEV

/** Large edit preview: progressive renders, before/after, crop tool and local-mask overlays. */
export function EditCanvas({ photo }: { photo: Photo }) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const stack = useEdit((s) => s.stack)
  const ready = useEdit((s) => s.loaded && s.photoId === photo.id)
  const dragging = useEdit((s) => s.dragging)
  const compare = useEdit((s) => s.compare)
  const splitPos = useEdit((s) => s.splitPos)
  const cropMode = useEdit((s) => s.cropMode)
  const maskOverlay = useEdit((s) => s.maskOverlay)
  const activeLocal = useEdit((s) => s.activeLocal)
  const clipping = useEdit((s) => s.clipping)
  const brush = useRepair((s) => s.brush)
  const hoverBystanders = useRepair((s) => s.hoverBystanders)
  const [box, size] = useElementSize<HTMLDivElement>()
  const [viewState, setViewState] = useState<{ key: string; view: ViewState }>({ key: '', view: FIT_VIEW })
  const viewKey = `${photo.id}:${cropMode}`
  const view = viewState.key === viewKey ? viewState.view : FIT_VIEW

  const crop = getCrop(stack)
  const main = useMemo(() => {
    const s = cropMode ? cropToolStack(stack) : stack
    return { body: { stack: s }, key: `${cropMode ? 'crop' : 'edit'}|${stackKey(s)}` }
  }, [stack, cropMode])
  const before = useMemo(() => {
    const b = beforeRequest(stack)
    return { body: b, key: `before|${b.stack ? stackKey(b.stack) : 'orig'}` }
  }, [stack])

  const mainFrame = usePreviewFrame({ photoId: photo.id, body: main.body, bodyKey: main.key, dragging, container: size, enabled: ready })
  const beforeFrame = usePreviewFrame({ photoId: photo.id, body: before.body, bodyKey: before.key, dragging, container: size, enabled: ready && compare !== 'off' && !cropMode })

  const showingOriginal = compare === 'original' && !cropMode
  const shown = showingOriginal ? (beforeFrame.frame ?? mainFrame.frame) : mainFrame.frame
  const imageSize = shown ? { w: shown.w, h: shown.h } : undefined
  const frameAspect = shown ? shown.w / shown.h : (photo.width ?? 3) / (photo.height ?? 2)
  const photoAspect = (photo.width ?? 3) / (photo.height ?? 2)

  const locals = localOps(stack)
  const activeOp = activeLocal !== null ? locals[activeLocal]?.op : undefined

  const overlay = (
    <>
      {compare === 'split' && !cropMode && (
        <SplitOverlay before={beforeFrame.frame?.url ?? null} pos={splitPos} onPos={(n) => useEdit.getState().setSplitPos(n)} />
      )}
      {cropMode && (
        <CropOverlay
          rect={crop?.rect ?? FULL_RECT}
          ratio={aspectRatio(crop?.aspect, photoAspect)}
          imgAspect={frameAspect}
          onLive={(rect) => setLive(setCrop(useEdit.getState().stack, { rect }))}
          onCommit={() => commitLive(qc, t('history.crop'))}
        />
      )}
      {!cropMode && !showingOriginal && maskOverlay && activeOp && <MaskOverlay photoId={photo.id} op={activeOp} aspect={frameAspect} crop={crop} />}
      {!cropMode && !showingOriginal && activeOp && activeLocal !== null && <MaskHandles index={activeLocal} op={activeOp} />}
      {!cropMode && !showingOriginal && hoverBystanders && <BystanderOverlay photoId={photo.id} crop={crop} aspect={photoAspect} />}
      {!cropMode && !showingOriginal && brush && <BrushOverlay crop={crop} aspect={photoAspect} />}
    </>
  )

  return (
    <div ref={box} className="relative h-full w-full" data-testid="edit-canvas">
      <ZoomPane
        photo={photo}
        view={view}
        onViewChange={(v) => setViewState({ key: viewKey, view: v })}
        src={shown?.url ?? null}
        imageSize={imageSize}
        overlay={overlay}
      />
      <div className="pointer-events-none absolute top-2 left-2 flex flex-col items-start gap-1">
        {showingOriginal && (
          <span className="rounded bg-black/65 px-1.5 py-0.5 text-xs font-medium text-white" data-testid="original-chip">
            {t('edit.original')}
          </span>
        )}
        {cropMode && <span className="rounded bg-accent px-1.5 py-0.5 text-xs font-semibold text-accent-fg">{t('edit.cropTool')}</span>}
        {clipping && <span className="rounded bg-black/65 px-1.5 py-0.5 text-xs text-white/85">{t('edit.clippingSoon')}</span>}
        {mainFrame.error && <span className="rounded bg-danger/90 px-1.5 py-0.5 text-xs text-white">{mainFrame.error}</span>}
      </div>
      {SHOW_BADGE && shown && (
        <div className="tnum pointer-events-none absolute bottom-2 left-2 rounded bg-black/55 px-1.5 py-0.5 font-mono text-[10px] text-white/60" data-testid="render-badge">
          {shown.backend ?? '?'} · {shown.renderMs ?? '?'}ms · {shown.w}×{shown.h} · {shown.final ? 'final' : 'drag'}
        </div>
      )}
    </div>
  )
}
