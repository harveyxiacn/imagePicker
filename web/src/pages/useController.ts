import { useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useMemo, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import type { ColorLabel, Flag, Photo } from '@/api/types'
import { acceptAiRatings, editPhotos, redo, undo } from '@/lib/actions'
import { jumpGroup, moveVertical, toggleStacks } from '@/lib/stacks'
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

/** View-specific behaviour injected by the page (group view needs the groups query). */
export interface ControllerHooks {
  /** Previous/next burst in the group view. */
  groupGo?: (dir: 1 | -1) => void
  /** Enter: pick A as the group's keeper; `next` also advances to the next group. */
  groupPick?: (next: boolean) => void
}

/** All user-facing photo operations. Reads store state lazily so callbacks stay stable. */
export function useController(photos: Photo[], hooks: ControllerHooks = {}) {
  const qc = useQueryClient()
  const { t } = useTranslation()
  const photosRef = useRef(photos)
  useEffect(() => {
    photosRef.current = photos
  })
  const push = useToasts((s) => s.push)
  const hooksRef = useRef(hooks)
  useEffect(() => {
    hooksRef.current = hooks
  })

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
      if ((ui.view === 'compare' || ui.view === 'group') && ui.compareA !== null && list[next]?.id === ui.compareA) {
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
      if (v === 'group') {
        // Enter the group view for the cursor photo's burst.
        const cur = list.find((p) => p.id === ui.activeId)
        if (ui.view === 'group') return
        if (!cur || cur.burst_id === null || (cur.burst_size ?? 0) < 2) {
          push('info', t('group.notInBurst'), 2500)
          return
        }
        ui.setGroupBurst(cur.burst_id)
        ui.setView('group')
        return
      }
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
    [indexOf, push, t],
  )

  const moveRow = useCallback(
    (dir: 1 | -1) => {
      const ui = useUi.getState()
      const id = moveVertical(ui.gridRows, ui.activeId, dir)
      if (id !== null) setActiveIndex(indexOf(id))
      else move(dir * ui.gridCols)
    },
    [indexOf, setActiveIndex, move],
  )

  const acceptAi = useCallback(async () => {
    const ids = targetIds()
    if (!ids.length) return
    const n = await acceptAiRatings(qc, ids, t('history.acceptAi'))
    push('info', n > 0 ? t('ai.accepted', { n }) : t('ai.nothingToAccept'), 2200)
  }, [qc, t, push, targetIds])

  const stacks = useCallback(
    (all = false) => {
      const ui = useUi.getState()
      const active = photosRef.current.find((p) => p.id === ui.activeId)
      const next = toggleStacks({ expandAll: ui.expandAll, expanded: ui.expandedStacks }, active, all)
      ui.setStacks(next)
      // The cursor must stay on a visible photo after a collapse: fall back to the stack cover.
      if (!next.expandAll && active?.burst_id != null && !next.expanded.has(active.burst_id) && (active.rank_in_burst ?? 0) > 0) {
        const cover = photosRef.current.find((p) => p.burst_id === active.burst_id && p.rank_in_burst === 0)
        if (cover) ui.setActive(cover.id)
      }
    },
    [],
  )

  const jump = useCallback((dir: 1 | -1) => {
    const ui = useUi.getState()
    if (ui.view === 'group') {
      hooksRef.current.groupGo?.(dir)
      return
    }
    const id = jumpGroup(photosRef.current, ui.activeId, dir)
    if (id !== null) setActiveIndex(photosRef.current.findIndex((p) => p.id === id))
  }, [setActiveIndex])

  const groupPick = useCallback((next: boolean) => hooksRef.current.groupPick?.(next), [])

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
    () => ({ move, moveRow, setActiveIndex, indexOf, rate, setFlag, setColor, undo: doUndo, redo: doRedo, setView, selectAll: doSelectAll, swapCompare, targetIds, acceptAi, stacks, jump, groupPick }),
    [move, moveRow, setActiveIndex, indexOf, rate, setFlag, setColor, doUndo, doRedo, setView, doSelectAll, swapCompare, targetIds, acceptAi, stacks, jump, groupPick],
  )
}

export type Controller = ReturnType<typeof useController>
