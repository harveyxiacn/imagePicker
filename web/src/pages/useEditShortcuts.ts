import { useEffect } from 'react'
import { dispatchKey, isTextEntryTarget, type Handlers } from '@/lib/keymap'
import { useEdit } from '@/stores/edit'
import { useUi } from '@/stores/ui'
import type { EditActions } from './useEditActions'

/** Keyboard handler for the edit page (scope `edit` of the central keymap). */
export function useEditShortcuts(actions: EditActions) {
  useEffect(() => {
    const handlers: Handlers = {
      'edit.undo': () => void actions.undo(),
      'edit.redo': () => void actions.redo(),
      'edit.before': () => actions.toggleOriginal(),
      'edit.mask': () => {
        const st = useEdit.getState()
        st.setMaskOverlay(!st.maskOverlay)
      },
      'edit.crop': () => {
        const st = useEdit.getState()
        st.setCropMode(!st.cropMode)
      },
      'edit.copy': () => actions.copy(),
      'edit.paste': () => void actions.paste(),
      'edit.close': () => {
        const st = useEdit.getState()
        if (st.cropMode) st.setCropMode(false)
        else if (st.historyOpen) st.setHistoryOpen(false)
        else if (st.compare !== 'off') st.setCompare('off')
        else actions.close()
      },
      'export.open': () => useUi.getState().setExportOpen(true),
      'help.toggle': () => useUi.getState().setHelpOpen(!useUi.getState().helpOpen),
    }
    const onKey = (e: KeyboardEvent) => {
      if (isTextEntryTarget(e.target)) return
      const dialogOpen = !!document.querySelector('[role="dialog"]')
      if (dialogOpen && e.key !== '?') return
      // one-shot actions ignore OS key repeat; undo / redo may repeat
      if (e.repeat && e.key.toLowerCase() !== 'z' && e.key.toLowerCase() !== 'y') return
      if (dispatchKey(e, 'edit', handlers)) e.preventDefault()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [actions])
}
