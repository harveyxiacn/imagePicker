import { keepPreviousData, useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useMemo } from 'react'
import { queryToFilter } from '@/lib/collections'
import { buildPhotosQuery, type FilterState } from '@/lib/filter'
import { qk } from '@/lib/cache'
import { mergeSettings } from '@/lib/settingsForm'
import { useToasts } from '@/stores/toasts'
import type { Settings, SettingsPatch } from './types'
import { api, fetchAllPhotos } from './client'


/**
 * Read-only guests get 403 for people, faces, taste, settings, assistant, system, fs, xmp, cache, onboarding,
 * masks, models, besttake and analysis: those queries wait for `/auth/me` and never run for a guest.
 */
function useNotGuest(): boolean {
  const me = useQuery({ queryKey: qk.me, queryFn: api.me, staleTime: 30_000, retry: false })
  return me.isSuccess && me.data.role !== 'guest'
}

export function useSessions() {
  return useQuery({ queryKey: qk.sessions, queryFn: api.sessions, select: (d) => d.sessions })
}

export function useSession(id: number) {
  return useQuery({
    queryKey: qk.session(id),
    queryFn: () => api.session(id),
    select: (d) => d.session,
  })
}

export function usePhotos(sessionId: number, filter: FilterState) {
  const query = useMemo(() => buildPhotosQuery(sessionId, filter), [sessionId, filter])
  return useQuery({
    queryKey: qk.photos(sessionId, query),
    queryFn: () => fetchAllPhotos(query),
    placeholderData: keepPreviousData,
  })
}

/** All photos of one burst (group view), independent of the library filters. */
export function useBurstPhotos(sessionId: number, burstId: number | null) {
  return useQuery({
    queryKey: qk.photos(sessionId, { session_id: sessionId, burst_id: burstId ?? -1 }),
    queryFn: () => fetchAllPhotos({ session_id: sessionId, burst_id: burstId ?? -1 }),
    enabled: burstId !== null,
    placeholderData: keepPreviousData,
  })
}

export function useHardware() {
  const ok = useNotGuest()
  return useQuery({ queryKey: qk.hardware, queryFn: api.hardware, select: (d) => d.worker, staleTime: 30_000, enabled: ok })
}

export function useModels(enabled = true) {
  const ok = useNotGuest()
  return useQuery({ queryKey: qk.models, queryFn: api.models, select: (d) => d.models, enabled: ok && enabled, staleTime: 0 })
}

export function useAnalysisStatus(sessionId: number) {
  const ok = useNotGuest()
  return useQuery({
    queryKey: qk.analysisStatus(sessionId),
    queryFn: () => api.analysisStatus(sessionId),
    staleTime: Infinity,
    enabled: ok,
  })
}

export function usePhotoAnalysis(photoId: number | undefined) {
  const ok = useNotGuest()
  return useQuery({
    queryKey: qk.analysis(photoId ?? -1),
    queryFn: () => api.photoAnalysis(photoId!),
    enabled: ok && (photoId !== undefined),
    placeholderData: keepPreviousData,
  })
}

export function useGroups(sessionId: number) {
  return useQuery({
    queryKey: qk.groups(sessionId),
    queryFn: () => api.groups(sessionId),
    select: (d) => d.scenes,
    staleTime: 30_000,
  })
}

export function useBurstFaces(burstId: number | null) {
  const ok = useNotGuest()
  return useQuery({
    queryKey: qk.burstFaces(burstId ?? -1),
    queryFn: () => api.burstFaces(burstId!),
    enabled: ok && (burstId !== null),
    placeholderData: keepPreviousData,
  })
}

export function usePeople(sessionId?: number) {
  // guests (LAN mode) have no access to people / face data
  const me = useQuery({ queryKey: qk.me, queryFn: api.me, staleTime: 30_000, retry: false })
  return useQuery({
    queryKey: qk.people(sessionId),
    queryFn: () => api.people(sessionId),
    select: (d) => d.people,
    staleTime: 15_000,
    enabled: me.isSuccess && me.data.role !== 'guest',
  })
}

export function useLuts() {
  return useQuery({ queryKey: qk.luts, queryFn: api.luts, select: (d) => d.luts, staleTime: 60_000 })
}

export function usePresets() {
  return useQuery({ queryKey: qk.presets, queryFn: api.presets, select: (d) => d.presets, staleTime: 60_000 })
}

/** People detected in one photo + whether their geometry is prepared (M4 portrait panel). */
export function usePhotoPeople(photoId: number | undefined, poll = false) {
  return useQuery({
    queryKey: qk.photoPeople(photoId ?? -1),
    queryFn: () => api.photoPeople(photoId!),
    enabled: photoId !== undefined,
    staleTime: 0,
    // Safety net while geometry is being prepared: `beauty.ready` normally arrives first.
    refetchInterval: poll ? 2500 : false,
  })
}

export function useCollections() {
  return useQuery({ queryKey: qk.collections, queryFn: api.collections, select: (d) => d.collections, staleTime: 30_000 })
}

/** Photo count of a collection query inside one session (lazy: only fetched while the row is mounted). */
export function useCollectionCount(sessionId: number, query: string, enabled = true) {
  return useQuery({
    queryKey: qk.collectionCount(sessionId, query),
    queryFn: () => api.photos({ ...buildPhotosQuery(sessionId, queryToFilter(query)), limit: 1 }),
    select: (d) => d.total,
    enabled,
    staleTime: 30_000,
  })
}

export function useTaste() {
  const ok = useNotGuest()
  return useQuery({ queryKey: qk.taste, queryFn: api.taste, staleTime: 15_000, enabled: ok })
}

/** Non-subject faces of a photo (M5 "消除路人"). */
export function useBystanders(photoId: number | undefined) {
  const ok = useNotGuest()
  return useQuery({
    queryKey: qk.bystanders(photoId ?? -1),
    queryFn: () => api.bystanders(photoId!),
    select: (d) => d.faces,
    enabled: ok && (photoId !== undefined),
    staleTime: 30_000,
  })
}

/** Best Take plan of a burst (M5): base photo + per-person candidates. */
export function useBestTakePlan(burstId: number | null) {
  const ok = useNotGuest()
  return useQuery({
    queryKey: qk.besttake(burstId ?? -1),
    queryFn: () => api.bestTakePlan(burstId!),
    enabled: ok && (burstId !== null),
    staleTime: 15_000,
  })
}

// ---- M6 ----

/** `/api/auth/me`: role + whether the server is in LAN mode. Never retried (a failure is handled by the 401 hook). */
export function useMe() {
  return useQuery({ queryKey: qk.me, queryFn: api.me, staleTime: 30_000, retry: false })
}

export function useSettings(enabled = true) {
  return useQuery({ queryKey: qk.settings, queryFn: api.settings, staleTime: 30_000, enabled })
}

export function useAssistantStatus(enabled = true) {
  return useQuery({ queryKey: qk.assistantStatus, queryFn: api.assistantStatus, staleTime: 30_000, enabled, retry: false })
}

export function useCacheInfo(enabled = true) {
  return useQuery({ queryKey: qk.cacheInfo, queryFn: api.cache, staleTime: 0, enabled })
}

export function useOnboarding(enabled = true) {
  return useQuery({ queryKey: qk.onboarding, queryFn: api.onboarding, staleTime: Infinity, enabled, retry: false })
}

export function useLan(enabled = true) {
  return useQuery({ queryKey: qk.lan, queryFn: api.lan, staleTime: 0, enabled, refetchInterval: (q) => (q.state.data?.restart_required ? 1000 : false) })
}

/** PATCH /api/settings with optimistic UI: the cache is patched at once, rolled back with a toast on failure. */
export function usePatchSettings() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (patch: SettingsPatch) => api.patchSettings(patch),
    onMutate: (patch) => {
      // cancel in-flight refetches (they would overwrite the optimistic value), then patch the cache synchronously
      void qc.cancelQueries({ queryKey: qk.settings })
      const prev = qc.getQueryData<Settings>(qk.settings)
      if (prev) qc.setQueryData(qk.settings, mergeSettings(prev, patch))
      return { prev }
    },
    onError: (e, _patch, ctx) => {
      if (ctx?.prev) qc.setQueryData(qk.settings, ctx.prev)
      useToasts.getState().push('error', e instanceof Error ? e.message : String(e), 6000)
    },
    onSuccess: (s) => qc.setQueryData(qk.settings, s),
  })
}
