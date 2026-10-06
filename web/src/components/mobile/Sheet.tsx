import * as Dialog from '@radix-ui/react-dialog'
import { X } from 'lucide-react'
import { useRef, useState, type PointerEvent as RPointerEvent, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'
import { settleSheet } from '@/lib/layout'

interface Props {
  open: boolean
  onOpenChange: (open: boolean) => void
  title: string
  children: ReactNode
  footer?: ReactNode
  /** Start expanded to (almost) full height; a flick up on the handle expands, a pull down closes. */
  tall?: boolean
  testId?: string
}

/**
 * Bottom sheet for phones (side panels, filter bar, inspector, pickers). Drag the handle down to dismiss,
 * flick it up to expand. Respects the home-indicator safe area.
 */
export function Sheet({ open, onOpenChange, title, children, footer, tall, testId }: Props) {
  const { t } = useTranslation()
  const [dy, setDy] = useState(0)
  const [expanded, setExpanded] = useState(false)
  const drag = useRef<{ y: number; t: number; h: number } | null>(null)
  const full = tall || expanded

  const down = (e: RPointerEvent<HTMLDivElement>) => {
    e.currentTarget.setPointerCapture(e.pointerId)
    drag.current = { y: e.clientY, t: performance.now(), h: e.currentTarget.parentElement?.getBoundingClientRect().height ?? 300 }
  }
  const move = (e: RPointerEvent) => {
    const d = drag.current
    if (d) setDy(Math.max(-40, e.clientY - d.y))
  }
  const up = (e: RPointerEvent) => {
    const d = drag.current
    drag.current = null
    if (!d) return
    const delta = e.clientY - d.y
    const v = delta / Math.max(1, performance.now() - d.t)
    setDy(0)
    const r = settleSheet(delta, v, d.h)
    if (r === 'close') onOpenChange(false)
    else if (r === 'expand') setExpanded(true)
  }

  return (
    <Dialog.Root
      open={open}
      onOpenChange={(o) => {
        if (!o) setExpanded(false)
        onOpenChange(o)
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="anim-fade fixed inset-0 z-40 bg-black/60" />
        <Dialog.Content
          data-testid={testId ?? 'sheet'}
          aria-describedby={undefined}
          className="sheet-in fixed right-0 bottom-0 left-0 z-50 mx-auto flex w-full max-w-2xl flex-col rounded-t-2xl border border-b-0 border-line bg-panel shadow-[var(--shadow)]"
          style={{
            maxHeight: full ? '94dvh' : '82dvh',
            height: full ? '94dvh' : undefined,
            transform: dy ? `translateY(${Math.max(0, dy)}px)` : undefined,
            transition: dy ? 'none' : 'transform 200ms var(--ease), height 200ms var(--ease)',
            paddingBottom: 'env(safe-area-inset-bottom, 0px)',
          }}
        >
          <div
            className="flex shrink-0 touch-none flex-col items-center pt-2"
            onPointerDown={down}
            onPointerMove={move}
            onPointerUp={up}
            onPointerCancel={up}
            data-testid="sheet-handle"
          >
            <span className="h-1.5 w-10 rounded-full bg-faint/60" />
            <div className="flex w-full items-center justify-between px-4 pt-1 pb-2">
              <Dialog.Title className="text-base font-semibold">{title}</Dialog.Title>
              <Dialog.Close className="btn btn-ghost btn-icon" aria-label={t('common.close')} onPointerDown={(e) => e.stopPropagation()}>
                <X size={18} />
              </Dialog.Close>
            </div>
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain border-t border-line">{children}</div>
          {footer && <div className="flex shrink-0 justify-end gap-2 border-t border-line px-4 py-3">{footer}</div>}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}
