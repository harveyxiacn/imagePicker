import { useCallback, useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import { useParams } from 'react-router-dom'
import { usePeople } from '@/api/queries'
import { displayName } from '@/lib/people'

/**
 * Text for a person given only their id and stored name (faces, burst tracks, best-take figures).
 * Unnamed people get the same "Unnamed N" the People page shows, never the internal cluster id.
 */
export function usePersonLabel(sessionId?: number): (id: number | null, name: string | null) => string {
  const { t } = useTranslation()
  const params = useParams()
  const sid = sessionId ?? (params.sessionId ? Number(params.sessionId) : undefined)
  const people = usePeople(sid)
  const numbers = useMemo(() => new Map((people.data ?? []).map((p) => [p.id, p.unnamed_no])), [people.data])
  const unnamed = t('person.unnamed')
  return useCallback((id, name) => displayName({ name, unnamed_no: id === null ? undefined : numbers.get(id) }, unnamed), [numbers, unnamed])
}
