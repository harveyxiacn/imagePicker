import type { ColorName, FlagFilter, Issue, PersonMode, PersonState, PhotosQuery, SceneType, SortKey } from '@/api/types'

/** Person filter (doc 04 3.6.1). Tri-state per person: absent = off, include, exclude. */
export interface PersonFilter {
  include: number[]
  exclude: number[]
  mode: PersonMode
  states: PersonState[]
  /** Subject-face count range; null = unbounded. `max: 0` = no people. */
  facesMin: number | null
  facesMax: number | null
  includeBackground: boolean
}

export const DEFAULT_PERSON_FILTER: PersonFilter = {
  include: [],
  exclude: [],
  mode: 'all',
  states: [],
  facesMin: null,
  facesMax: null,
  includeBackground: false,
}

export interface FilterState {
  /** Minimum user rating (0 = no rating filter). */
  ratingGte: number
  /** Minimum AI rating (0 = off). */
  aiRatingGte: number
  flag: 'all' | FlagFilter
  color: ColorName | null
  /** `none` = only photos without issues; `any` = only photos with one of `issues` (or any issue when empty). */
  issueMode: 'all' | 'none' | 'any'
  issues: Issue[]
  sceneType: SceneType | null
  person: PersonFilter
  sort: SortKey
}

export const DEFAULT_FILTER: FilterState = {
  ratingGte: 0,
  aiRatingGte: 0,
  flag: 'all',
  color: null,
  issueMode: 'all',
  issues: [],
  sceneType: null,
  person: DEFAULT_PERSON_FILTER,
  sort: 'taken_at',
}

export type PhotosListQuery = Omit<PhotosQuery, 'cursor' | 'limit'>

const ALL_ISSUES: Issue[] = ['closed_eyes', 'blurry', 'overexposed', 'underexposed', 'noisy', 'tilted']

/** Translate UI filter state to /api/photos query params. Defaults are omitted. */
export function buildPhotosQuery(sessionId: number, f: FilterState): PhotosListQuery {
  const q: PhotosListQuery = { session_id: sessionId }
  if (f.ratingGte > 0) q.rating_gte = f.ratingGte
  if (f.aiRatingGte > 0) q.ai_rating_gte = f.aiRatingGte
  if (f.flag !== 'all') q.flag = f.flag
  if (f.color) q.color_label = f.color
  if (f.issueMode === 'none') q.issues_none = true
  else if (f.issueMode === 'any') q.issues_any = f.issues.length ? [...f.issues] : [...ALL_ISSUES]
  if (f.sceneType) q.scene_type = f.sceneType
  const p = f.person
  if (p.include.length) {
    q.persons = [...p.include]
    if (p.mode === 'any') q.person_mode = 'any'
    if (p.states.length) q.person_state = [...p.states]
  }
  if (p.exclude.length) q.exclude_persons = [...p.exclude]
  if (p.includeBackground && (p.include.length || p.exclude.length)) q.include_background = true
  if (p.facesMin !== null) q.faces_min = p.facesMin
  if (p.facesMax !== null) q.faces_max = p.facesMax
  if (f.sort !== DEFAULT_FILTER.sort) q.sort = f.sort
  return q
}

export function isPersonFilterActive(p: PersonFilter): boolean {
  return p.include.length > 0 || p.exclude.length > 0 || p.facesMin !== null || p.facesMax !== null
}

export function isFilterActive(f: FilterState): boolean {
  return (
    f.ratingGte > 0 ||
    f.aiRatingGte > 0 ||
    f.flag !== 'all' ||
    f.color !== null ||
    f.issueMode !== 'all' ||
    f.sceneType !== null ||
    isPersonFilterActive(f.person)
  )
}

export type PersonTri = 'off' | 'include' | 'exclude'

export function personTri(p: PersonFilter, id: number): PersonTri {
  return p.include.includes(id) ? 'include' : p.exclude.includes(id) ? 'exclude' : 'off'
}

/** Click cycle: off -> include -> exclude -> off. */
export function cyclePerson(p: PersonFilter, id: number): PersonFilter {
  switch (personTri(p, id)) {
    case 'off':
      return { ...p, include: [...p.include, id] }
    case 'include':
      return { ...p, include: p.include.filter((x) => x !== id), exclude: [...p.exclude, id] }
    default:
      return { ...p, exclude: p.exclude.filter((x) => x !== id) }
  }
}

/** Force a person into a tri-state (used by "filter by this face" / "exclude"). */
export function setPersonTri(p: PersonFilter, id: number, tri: PersonTri): PersonFilter {
  const include = p.include.filter((x) => x !== id)
  const exclude = p.exclude.filter((x) => x !== id)
  if (tri === 'include') include.push(id)
  if (tri === 'exclude') exclude.push(id)
  return { ...p, include, exclude }
}

export function toggleState(p: PersonFilter, s: PersonState): PersonFilter {
  return { ...p, states: p.states.includes(s) ? p.states.filter((x) => x !== s) : [...p.states, s] }
}

export type FacesPreset = 'any' | 'single' | 'few' | 'many' | 'none'

export function facesPreset(p: PersonFilter): FacesPreset | 'custom' {
  const { facesMin: lo, facesMax: hi } = p
  if (lo === null && hi === null) return 'any'
  if (lo === 1 && hi === 1) return 'single'
  if (lo === 2 && hi === 3) return 'few'
  if (lo === null && hi === 0) return 'none'
  if (lo !== null && lo >= 4 && hi === null) return 'many'
  return 'custom'
}

export function applyFacesPreset(p: PersonFilter, preset: FacesPreset, manyMin = 4): PersonFilter {
  switch (preset) {
    case 'any':
      return { ...p, facesMin: null, facesMax: null }
    case 'single':
      return { ...p, facesMin: 1, facesMax: 1 }
    case 'few':
      return { ...p, facesMin: 2, facesMax: 3 }
    case 'many':
      return { ...p, facesMin: manyMin, facesMax: null }
    case 'none':
      return { ...p, facesMin: null, facesMax: 0 }
  }
}
