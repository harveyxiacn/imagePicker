import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { EDIT_SECTIONS, type EditSection } from '@/api/types'
import { extractSections, sectionsPresent } from '@/lib/edit'
import { useEdit } from '@/stores/edit'
import { useToasts } from '@/stores/toasts'
import { Modal } from '../Modal'

/** Ctrl+Shift+C: choose which sections of the current stack go on the clipboard. */
export function CopyDialog() {
  const { t } = useTranslation()
  const open = useEdit((s) => s.copyDialog)
  const setOpen = useEdit((s) => s.setCopyDialog)
  return (
    <Modal open={open} onOpenChange={setOpen} title={t('edit.copyTitle')} description={t('edit.copyDesc')} width="max-w-sm">
      {open && <CopyBody onDone={() => setOpen(false)} />}
    </Modal>
  )
}

function CopyBody({ onDone }: { onDone: () => void }) {
  const { t } = useTranslation()
  const stack = useEdit.getState().stack
  const present = sectionsPresent(stack)
  // Like Lightroom, geometry is opt-in.
  const [picked, setPicked] = useState<EditSection[]>(present.filter((s) => s !== 'crop'))

  const toggle = (s: EditSection) => setPicked((p) => (p.includes(s) ? p.filter((x) => x !== s) : [...p, s]))
  const copy = () => {
    const st = useEdit.getState()
    if (st.photoId === null) return
    st.setClipboard({ stack: extractSections(st.stack, picked), sections: picked, fromId: st.photoId })
    useToasts.getState().push('info', t('edit.copied', { n: picked.length }), 2000)
    onDone()
  }
  return (
    <div className="flex flex-col gap-2" data-testid="copy-dialog">
      {present.length === 0 && <div className="text-muted">{t('edit.copyNothing')}</div>}
      {EDIT_SECTIONS.map((s) => (
        <label key={s} className={`flex items-center gap-2 ${present.includes(s) ? '' : 'opacity-40'}`}>
          <input type="checkbox" disabled={!present.includes(s)} checked={picked.includes(s)} onChange={() => toggle(s)} data-testid={`copy-${s}`} />
          {t(`edit.section_${s}`)}
        </label>
      ))}
      <div className="mt-2 flex justify-end gap-2">
        <button className="btn" onClick={onDone}>
          {t('common.cancel')}
        </button>
        <button className="btn btn-primary" disabled={picked.length === 0} onClick={copy} data-testid="copy-confirm">
          {t('edit.copyConfirm')}
        </button>
      </div>
    </div>
  )
}
