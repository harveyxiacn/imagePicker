/**
 * Smart collections (docs/api-contract-m4.md C.2): a collection stores the /api/photos filter params as a
 * URLSearchParams string. These helpers convert between that string and the UI `FilterState`.
 */
import { photosQueryString } from '@/api/client'
import { ISSUES, SCENE_TYPES, type ColorName, type FlagFilter, type Issue, type PersonState, type SceneType, type SortKey } from '@/api/types'
import { buildPhotosQuery, DEFAULT_FILTER, DEFAULT_PERSON_FILTER, type FilterState } from './filter'

const COLORS: ColorName[] = ['red', 'yellow', 'green', 'blue', 'purple']
const FLAGS: FlagFilter[] = ['picked', 'rejected', 'unflagged', 'not_rejected']
const SORTS: SortKey[] = ['taken_at', '-taken_at', 'name', 'rating', 'ai']
const STATES: PersonState[] = ['eyes_open', 'smiling', 'looking', 'subject']

/** Serialise the current filter (session independent: no session_id / cursor / limit). */
export function filterToQuery(f: FilterState): string {
  const p = new URLSearchParams(photosQueryString(buildPhotosQuery(0, f)))
  p.delete('session_id')
  return p.toString()
}

const csv = (v: string | null): string[] => (v ? v.split(',').filter(Boolean) : [])
const nums = (v: string | null): number[] =>
  csv(v)
    .map(Number)
    .filter((n) => Number.isFinite(n))
const num = (v: string | null): number | null => {
  if (v === null || v === '') return null
  const n = Number(v)
  return Number.isFinite(n) ? n : null
}
const oneOf = <T extends string>(v: string | null, all: readonly T[]): T | null => (v !== null && (all as readonly string[]).includes(v) ? (v as T) : null)

/** Parse a stored collection query into a full filter state (unknown params are ignored). */
export function queryToFilter(query: string): FilterState {
  const q = new URLSearchParams(query.replace(/^\?/, ''))
  const issuesAny = csv(q.get('issues_any')).filter((i): i is Issue => (ISSUES as string[]).includes(i))
  const include = nums(q.get('persons'))
  const exclude = nums(q.get('exclude_persons'))
  const rating = num(q.get('rating_gte'))
  const aiRating = num(q.get('ai_rating_gte'))
  const none = q.get('issues_none') === '1'
  return {
    ...DEFAULT_FILTER,
    ratingGte: rating !== null && rating > 0 ? rating : 0,
    aiRatingGte: aiRating !== null && aiRating > 0 ? aiRating : 0,
    flag: oneOf(q.get('flag'), FLAGS) ?? 'all',
    color: oneOf<ColorName>(q.get('color_label'), COLORS),
    issueMode: none ? 'none' : issuesAny.length ? 'any' : 'all',
    // "every issue" is how an empty selection serialises, so it parses back to the empty (= any) selection
    issues: none || issuesAny.length === ISSUES.length ? [] : issuesAny,
    sceneType: oneOf<SceneType>(q.get('scene_type'), SCENE_TYPES),
    bestOnly: q.get('burst_best_only') === '1',
    edited: q.get('has_edits') === '1',
    person: {
      ...DEFAULT_PERSON_FILTER,
      include,
      exclude,
      mode: q.get('person_mode') === 'any' ? 'any' : 'all',
      states: include.length ? csv(q.get('person_state')).filter((s): s is PersonState => (STATES as string[]).includes(s)) : [],
      facesMin: num(q.get('faces_min')),
      facesMax: num(q.get('faces_max')),
      includeBackground: q.get('include_background') === '1' && (include.length > 0 || exclude.length > 0),
    },
    sort: oneOf<SortKey>(q.get('sort'), SORTS) ?? DEFAULT_FILTER.sort,
  }
}

/** Canonical form of a stored query (parse + serialise), for comparing with the live filter. */
export const canonicalQuery = (query: string): string => filterToQuery(queryToFilter(query))

/** Does the live filter equal this collection? */
export const collectionActive = (query: string, f: FilterState): boolean => canonicalQuery(query) === filterToQuery(f)
