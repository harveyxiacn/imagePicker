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
  const { ratingGte, flag, color, sort } = filter
  const query = useMemo(
    () => buildPhotosQuery(sessionId, { ratingGte, flag, color, sort }),
    [sessionId, ratingGte, flag, color, sort],
  )
  return useQuery({
    queryKey: qk.photos(sessionId, query),
    queryFn: () => fetchAllPhotos(query),
    placeholderData: keepPreviousData,
  })
}
