import { keepPreviousData, useQuery } from '@tanstack/react-query'
import { useMemo } from 'react'
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

export function usePresets() {
  return useQuery({ queryKey: qk.presets, queryFn: api.presets, select: (d) => d.presets, staleTime: 60_000 })
}
