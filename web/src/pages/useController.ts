import { useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useMemo, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import type { ColorLabel, Flag, Photo } from '@/api/types'
import { editPhotos, redo, undo } from '@/lib/actions'
import { clickSelect, selectAll } from '@/lib/selection'
import { useToasts } from '@/stores/toasts'
import { useUi, type View } from '@/stores/ui'

/** Photos shown in compare mode: pinned A, active B, then following candidates. */
export function compareSlots(photos: Photo[], aId: number | null, activeId: number | null, count: 2 | 4): Photo[] {
  const byId = (id: number | null) => (id === null ? undefined : photos.find((p) => p.id === id))
  const a = byId(aId)
  const b = byId(activeId)
  const out: Photo[] = []
  if (a) out.push(a)
  if (b && b !== a) out.push(b)
  if (b) {
    let i = photos.indexOf(b) + 1
    while (out.length < count && i < photos.length) {
      if (photos[i] !== a) out.push(photos[i])
      i++
    }
  }
  return out.slice(0, count)
}

/** All user-facing photo operations. Reads store state lazily so callbacks stay stable. */
export function useController(photos: Photo[]) {
  const qc = useQueryClient()
  const { t } = useTranslation()
  const photosRef = useRef(photos)
  useEffect(() => {
    photosRef.current = photos
  })
  const push = useToasts((s) => s.push)

  const indexOf = useCallback((id: number | null) => (id === null ? -1 : photosRef.current.findIndex((p) => p.id === id)), [])

  const setActiveIndex = useCallback((i: number, opts?: { extend?: boolean }) => {
    const list = photosRef.current
    if (!list.length) return
    const idx = Math.min(list.length - 1, Math.max(0, i))
    const id = list[idx].id
    const ui = useUi.getState()
    ui.setActive(id)
    if (ui.view === 'grid') {
      ui.setSelection(
        clickSelect(ui.selection, list.map((p) => p.id), id, { shift: opts?.extend }),
      )
    }
  }, [])

  const move = useCallback(
    (delta: number, opts?: { extend?: boolean }) => {
      const ui = useUi.getState()
      const list = photosRef.current
      let i = indexOf(ui.activeId)
      if (i < 0) i = delta > 0 ? -1 : list.length
      let next = Math.min(list.length - 1, Math.max(0, i + delta))
      if (ui.view === 'compare' && ui.compareA !== null && list[next]?.id === ui.compareA) {
        const skip = next + Math.sign(delta || 1)
        if (skip >= 0 && skip < list.length) next = skip
      }
      setActiveIndex(next, opts)
    },
    [indexOf, setActiveIndex],
  )

  const targetIds = useCallback((): number[] => {
    const ui = useUi.getState()
    if (ui.view === 'grid' && ui.selection.ids.size > 0) return [...ui.selection.ids]
    return ui.activeId !== null ? [ui.activeId] : []
  }, [])

  const rate = useCallback(
    (n: number, advance = false) => {
      const ids = targetIds()
      if (!ids.length) return
      void editPhotos(qc, ids, { user_rating: n === 0 ? null : n }, t('history.rating'))
      if (advance) move(1)
    },
    [qc, t, targetIds, move],
  )

  const setFlag = useCallback(
    (f: Flag) => {
      const ids = targetIds()
      if (ids.length) void editPhotos(qc, ids, { flag: f }, t('history.flag'))
    },
    [qc, t, targetIds],
  )

  const setColor = useCallback(
    (c: ColorLabel) => {
      const ids = targetIds()
      if (!ids.length) return
      // Toggle off when every target already carries this label.
      const current = photosRef.current.filter((p) => ids.includes(p.id))
      const next = c !== null && current.length > 0 && current.every((p) => p.color_label === c) ? null : c
      void editPhotos(qc, ids, { color_label: next }, t('history.color'))
    },
    [qc, t, targetIds],
  )

  const doUndo = useCallback(async () => {
    const e = await undo(qc)
    push('info', e ? t('history.undone', { label: e.label }) : t('history.nothingToUndo'), 1800)
  }, [qc, t, push])
  const doRedo = useCallback(async () => {
    const e = await redo(qc)
    push('info', e ? t('history.redone', { label: e.label }) : t('history.nothingToRedo'), 1800)
  }, [qc, t, push])

  const setView = useCallback(
    (v: View) => {
      const ui = useUi.getState()
      const list = photosRef.current
      if (v === 'compare' && ui.view !== 'compare') {
        // Pin the current photo as A and move the cursor to the next candidate.
        const i = indexOf(ui.activeId)
        if (i >= 0) {
          ui.setCompareA(list[i].id)
          if (list.length > 1) ui.setActive(list[i + 1 < list.length ? i + 1 : i - 1].id)
        }
      }
      if (v === 'loupe' && ui.activeId === null && list.length) ui.setActive(list[0].id)
      ui.setView(v)
    },
    [indexOf],
  )

  const doSelectAll = useCallback(() => {
    const ui = useUi.getState()
    ui.setSelection(selectAll(photosRef.current.map((p) => p.id), ui.activeId))
  }, [])

  const swapCompare = useCallback(() => {
    const ui = useUi.getState()
    if (ui.compareA !== null && ui.activeId !== null) {
      const a = ui.compareA
      ui.setCompareA(ui.activeId)
      ui.setActive(a)
    }
  }, [])

  return useMemo(
    () => ({ move, setActiveIndex, indexOf, rate, setFlag, setColor, undo: doUndo, redo: doRedo, setView, selectAll: doSelectAll, swapCompare, targetIds }),
    [move, setActiveIndex, indexOf, rate, setFlag, setColor, doUndo, doRedo, setView, doSelectAll, swapCompare, targetIds],
  )
}

export type Controller = ReturnType<typeof useController>
