import { keepPreviousData, useQuery } from '@tanstack/react-query'
import { useMemo } from 'react'
import { queryToFilter } from '@/lib/collections'
import { buildPhotosQuery, type FilterState } from '@/lib/filter'
import { qk } from '@/lib/cache'
import { api, fetchAllPhotos } from './client'

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
  return useQuery({ queryKey: qk.hardware, queryFn: api.hardware, select: (d) => d.worker, staleTime: 30_000 })
}

export function useModels(enabled = true) {
  return useQuery({ queryKey: qk.models, queryFn: api.models, select: (d) => d.models, enabled, staleTime: 0 })
}

export function useAnalysisStatus(sessionId: number) {
  return useQuery({
    queryKey: qk.analysisStatus(sessionId),
    queryFn: () => api.analysisStatus(sessionId),
    staleTime: Infinity,
  })
}

export function usePhotoAnalysis(photoId: number | undefined) {
  return useQuery({
    queryKey: qk.analysis(photoId ?? -1),
    queryFn: () => api.photoAnalysis(photoId!),
    enabled: photoId !== undefined,
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
  return useQuery({
    queryKey: qk.burstFaces(burstId ?? -1),
    queryFn: () => api.burstFaces(burstId!),
    enabled: burstId !== null,
    placeholderData: keepPreviousData,
  })
}

export function usePeople(sessionId?: number) {
  return useQuery({
    queryKey: qk.people(sessionId),
    queryFn: () => api.people(sessionId),
    select: (d) => d.people,
    staleTime: 15_000,
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
  return useQuery({ queryKey: qk.taste, queryFn: api.taste, staleTime: 15_000 })
}
