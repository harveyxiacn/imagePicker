import { useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useMemo, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import { api, fetchAllPhotos } from '@/api/client'
import { useBurstPhotos, useGroups } from '@/api/queries'
import type { Burst, Photo, Scene } from '@/api/types'
import { editPhotoGroups } from '@/lib/actions'
import { qk } from '@/lib/cache'
import { useHistory } from '@/lib/history'
import { mergeGroupsEntry, splitGroupEntry } from '@/lib/structureUndo'
import { useToasts } from '@/stores/toasts'
import { useUi } from '@/stores/ui'

const timeOrder = (a: Photo, b: Photo) => (a.taken_at ?? 0) - (b.taken_at ?? 0) || a.id - b.id

/** State + actions of the group view (B): the current burst, its photos in shot order, navigation, keep-best, split/merge. */
export function useGroup(sessionId: number) {
  const qc = useQueryClient()
  const { t } = useTranslation()
  const push = useToasts((s) => s.push)
  const view = useUi((s) => s.view)
  const burstId = useUi((s) => s.groupBurstId)
  const compareA = useUi((s) => s.compareA)
  const activeId = useUi((s) => s.activeId)
  const scenesQ = useGroups(sessionId)
  const scenes: Scene[] | undefined = scenesQ.data
  const active = view === 'group'

  const allBursts = useMemo(() => (scenes ?? []).flatMap((s) => s.bursts), [scenes])
  const multi = useMemo(() => allBursts.filter((b) => b.size > 1), [allBursts])
  const burst: Burst | undefined = allBursts.find((b) => b.id === burstId)
  const index = multi.findIndex((b) => b.id === burstId)

  const q = useBurstPhotos(sessionId, active ? burstId : null)
  const photos = useMemo(() => (q.isPlaceholderData || !q.data ? [] : [...q.data.photos].sort(timeOrder)), [q.data, q.isPlaceholderData])

  // Initialise A (best shot) / B (next candidate) once per burst entry.
  const inited = useRef<number | null>(null)
  useEffect(() => {
    if (!active) {
      inited.current = null
      return
    }
    if (burstId === null || photos.length === 0 || inited.current === burstId) return
    inited.current = burstId
    const best = photos.find((p) => p.rank_in_burst === 0) ?? photos[0]
    const ui = useUi.getState()
    const cur = photos.find((p) => p.id === ui.activeId)
    const b = cur && cur.id !== best.id ? cur : (photos.find((p) => p.id !== best.id) ?? best)
    ui.setCompareA(best.id)
    ui.setActive(b.id)
  }, [active, burstId, photos])

  /** After a group split / merge (or its undo / redo): refresh the caches and keep the group view on a group that still exists. */
  const refreshGroups = useCallback(
    (focus?: number) => {
      void qc.invalidateQueries({ queryKey: qk.groups(sessionId) })
      // only the per-burst photo lists (the library grid does not change)
      void qc.invalidateQueries({ queryKey: qk.photosAll, predicate: (q) => (q.queryKey[2] as { burst_id?: number } | undefined)?.burst_id !== undefined })
      if (focus !== undefined && useUi.getState().view === 'group') useUi.getState().setGroupBurst(focus)
    },
    [qc, sessionId],
  )

  const go = useCallback(
    (dir: 1 | -1) => {
      const next = multi[index + dir]
      if (next) useUi.getState().setGroupBurst(next.id)
      else push('info', t(dir > 0 ? 'group.lastGroup' : 'group.firstGroup'), 1800)
    },
    [multi, index, push, t],
  )

  const pickA = useCallback(
    (next: boolean) => {
      const a = useUi.getState().compareA
      if (a !== null) void editPhotoGroups(qc, [{ ids: [a], patch: { flag: 1 } }], t('history.pick'))
      if (next) go(1)
    },
    [qc, t, go],
  )

  const keepBest = useCallback(async () => {
    const a = useUi.getState().compareA ?? photos[0]?.id
    if (a === undefined || a === null) return
    const others = photos.filter((p) => p.id !== a).map((p) => p.id)
    await editPhotoGroups(
      qc,
      [
        { ids: [a], patch: { flag: 1 } },
        { ids: others, patch: { flag: -1 } },
      ],
      t('history.keepBest'),
    )
    push('success', t('group.keptBest', { n: others.length }), 2600)
  }, [qc, t, push, photos])

  const split = useCallback(async () => {
    const ui = useUi.getState()
    if (burstId === null || ui.activeId === null) return
    const at = photos.findIndex((p) => p.id === ui.activeId)
    if (at <= 0) {
      push('info', t('group.cannotSplit'), 2200)
      return
    }
    try {
      const { burst_ids } = await api.splitGroup(burstId, ui.activeId)
      const aIdx = photos.findIndex((p) => p.id === ui.compareA)
      ui.setGroupBurst(aIdx >= at ? burst_ids[1] : burst_ids[0])
      refreshGroups()
      useHistory.getState().push(splitGroupEntry({ label: t('history.splitGroup'), api, burstIds: burst_ids, atPhotoId: ui.activeId, after: refreshGroups }))
      push('success', t('group.splitDone'), 2200)
    } catch (err) {
      push('error', err instanceof Error ? err.message : String(err))
    }
  }, [burstId, photos, refreshGroups, push, t])

  const merge = useCallback(
    async (dir: 1 | -1) => {
      if (burstId === null) return
      const i = allBursts.findIndex((b) => b.id === burstId)
      const other = allBursts[i + dir]
      if (!other) {
        push('info', t('group.nothingToMerge'), 2200)
        return
      }
      try {
        // The later group (in shot order) starts where an undo has to split again; the server keeps the earlier id.
        const mine = photos[0]
        const theirs = [...(await fetchAllPhotos({ session_id: sessionId, burst_id: other.id })).photos].sort(timeOrder)[0]
        const splitAt = mine && theirs ? (timeOrder(theirs, mine) > 0 ? theirs : mine) : undefined
        const { burst_id } = await api.mergeGroups([burstId, other.id])
        useUi.getState().setGroupBurst(burst_id)
        refreshGroups()
        if (splitAt) useHistory.getState().push(mergeGroupsEntry({ label: t('history.mergeGroups'), api, mergedId: burst_id, splitAtPhotoId: splitAt.id, after: refreshGroups }))
        push('success', t('group.mergeDone'), 2200)
      } catch (err) {
        push('error', err instanceof Error ? err.message : String(err))
      }
    },
    [burstId, allBursts, photos, sessionId, refreshGroups, push, t],
  )

  return { burstId, burst, index, count: multi.length, photos, loading: q.isPending || q.isPlaceholderData, compareA, activeId, go, pickA, keepBest, split, merge }
}

export type GroupApi = ReturnType<typeof useGroup>
