import * as Dialog from '@radix-ui/react-dialog'
import { X } from 'lucide-react'
import type { ReactNode } from 'react'
import { useTranslation } from 'react-i18next'

interface Props {
  open: boolean
  onOpenChange: (open: boolean) => void
  title: string
  description?: string
  children: ReactNode
  footer?: ReactNode
  width?: string
}

export function Modal({ open, onOpenChange, title, description, children, footer, width = 'max-w-lg' }: Props) {
  const { t } = useTranslation()
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="anim-fade fixed inset-0 z-40 bg-black/60" />
        <Dialog.Content
          className={`anim-pop fixed top-1/2 left-1/2 z-50 flex max-h-[88vh] w-[calc(100vw-32px)] -translate-x-1/2 -translate-y-1/2 flex-col rounded-card border border-line bg-panel shadow-[var(--shadow)] ${width}`}
        >
          <div className="flex items-center justify-between border-b border-line px-4 py-3">
            <Dialog.Title className="text-base font-semibold">{title}</Dialog.Title>
            <Dialog.Close className="btn btn-ghost btn-icon" aria-label={t('common.close')}>
              <X size={16} />
            </Dialog.Close>
          </div>
          {description ? (
            <Dialog.Description className="px-4 pt-3 text-muted">{description}</Dialog.Description>
          ) : (
            <Dialog.Description className="sr-only">{title}</Dialog.Description>
          )}
          <div className="min-h-0 flex-1 overflow-auto p-4">{children}</div>
          {footer && <div className="flex justify-end gap-2 border-t border-line px-4 py-3">{footer}</div>}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}
