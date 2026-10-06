import { useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useMemo, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import { ApiError } from '@/api/client'
import type { Photo } from '@/api/types'
import { redo, undo } from '@/lib/actions'
import { resetAll, pasteToPhotos, syncToPhotos } from '@/lib/editActions'
import { useEdit } from '@/stores/edit'
import { useToasts } from '@/stores/toasts'
import { useUi } from '@/stores/ui'

/** Commands of the edit page shared by keyboard shortcuts and toolbar buttons. */
export function useEditActions(photo: Photo | undefined, photos: Photo[], onClose: () => void) {
  const qc = useQueryClient()
  const { t } = useTranslation()
  const push = useToasts((s) => s.push)
  const latest = useRef({ photo, photos, onClose })
  useEffect(() => {
    latest.current = { photo, photos, onClose }
  })

  const fail = useCallback((err: unknown) => push('error', err instanceof ApiError || err instanceof Error ? err.message : String(err), 5000), [push])

  const doUndo = useCallback(async () => {
    const e = await undo(qc)
    push('info', e ? t('history.undone', { label: e.label }) : t('history.nothingToUndo'), 1800)
  }, [qc, t, push])
  const doRedo = useCallback(async () => {
    const e = await redo(qc)
    push('info', e ? t('history.redone', { label: e.label }) : t('history.nothingToRedo'), 1800)
  }, [qc, t, push])

  const toggleOriginal = useCallback(() => {
    const st = useEdit.getState()
    if (st.cropMode) return
    st.setCompare(st.compare === 'original' ? 'off' : 'original')
  }, [])
  const toggleSplit = useCallback(() => {
    const st = useEdit.getState()
    if (st.cropMode) return
    st.setCompare(st.compare === 'split' ? 'off' : 'split')
  }, [])

  const copy = useCallback(() => useEdit.getState().setCopyDialog(true), [])

  const paste = useCallback(async () => {
    const clip = useEdit.getState().clipboard
    const { photo: p } = latest.current
    if (!clip) {
      push('info', t('edit.pasteNothing'), 2200)
      return
    }
    const sel = [...useUi.getState().selection.ids]
    // A multi-selection wins; a stray single selection from the library must not hijack the open photo.
    const targets = sel.length > 1 ? sel : p ? [p.id] : []
    try {
      const n = await pasteToPhotos(qc, targets, clip, t('history.paste', { n: targets.length }))
      push('info', t('edit.pasted', { n }), 2200)
    } catch (err) {
      fail(err)
    }
  }, [qc, t, push, fail])

  /** Photos "in the same group": burst mates of the current photo, else the multi-selection. */
  const groupIds = useCallback((): number[] => {
    const { photo: p, photos: list } = latest.current
    if (!p) return []
    const mates = p.burst_id !== null ? list.filter((x) => x.burst_id === p.burst_id && x.id !== p.id).map((x) => x.id) : []
    if (mates.length) return mates
    const sel = [...useUi.getState().selection.ids]
    return sel.length > 1 ? sel.filter((id) => id !== p.id) : []
  }, [])

  const sync = useCallback(async () => {
    const { photo: p } = latest.current
    if (!p) return
    const ids = groupIds()
    if (!ids.length) {
      push('info', t('edit.syncNone'), 2500)
      return
    }
    try {
      const n = await syncToPhotos(qc, p.id, ids, ['global', 'local', 'lut', 'output_sharpen'], t('history.sync', { n: ids.length }))
      push('success', t('edit.synced', { n }), 2500)
    } catch (err) {
      fail(err)
    }
  }, [qc, t, push, fail, groupIds])

  const resetEverything = useCallback(() => {
    resetAll(qc, t('history.resetAll'))
  }, [qc, t])

  return useMemo(
    () => ({ undo: doUndo, redo: doRedo, toggleOriginal, toggleSplit, copy, paste, sync, groupIds, resetAll: resetEverything, close: () => latest.current.onClose() }),
    [doUndo, doRedo, toggleOriginal, toggleSplit, copy, paste, sync, groupIds, resetEverything],
  )
}

export type EditActions = ReturnType<typeof useEditActions>
