import { normalizeSearch } from './search'
import type { ColorName, FlagFilter, Issue, PersonMode, PersonState, PhotosQuery, SceneType, SharpState, SortKey } from '@/api/types'

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

/** Similarity (cosine) of the "similar faces" filter when none is chosen; the server's clustering threshold. */
export const DEFAULT_FACE_SIMILARITY = 0.38
export const FACE_SIMILARITY_MIN = 0.2
export const FACE_SIMILARITY_MAX = 0.8

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
  /** Camera / phone device ids, or 'none' for photos without EXIF camera. */
  device: Array<number | 'none'>
  /** Exact lens names (any of them) */
  lens: string[]
  /** Capture date range, `YYYY-MM-DD` (inclusive, wall clock); null = open */
  takenFrom: string | null
  takenTo: string | null
  /** Keywords; `tagMode`: any of them, or all of them */
  tags: string[]
  tagMode: 'any' | 'all'
  /** Sharpness state from the analysis; null = off */
  sharp: SharpState | null
  /** Photos with a face similar to one of these faces; `faceSimilarity` null = default threshold */
  faceIds: number[]
  faceSimilarity: number | null
  /** M4: only the best shot of every burst (built-in collection "each group's best") */
  bestOnly: boolean
  /** M4: only photos that carry saved edits */
  edited: boolean
  /** Only photos that have a duplicate in the catalog (built-in "duplicates" collection) */
  duplicates: boolean
  /** Only the photos of one event of the session timeline; session specific, never saved in a collection */
  eventId: number | null
  /** Natural-language search text (GET /api/search); '' = off. Never saved in a collection. */
  search: string
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
  device: [],
  lens: [],
  takenFrom: null,
  takenTo: null,
  tags: [],
  tagMode: 'any',
  sharp: null,
  faceIds: [],
  faceSimilarity: null,
  bestOnly: false,
  edited: false,
  duplicates: false,
  eventId: null,
  search: '',
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
  if (f.bestOnly) q.burst_best_only = true
  if (f.edited) q.has_edits = true
  if (f.duplicates) q.duplicates = true
  if (typeof f.eventId === 'number') q.event_id = f.eventId
  if (f.device.length) q.device = [...f.device]
  if (f.lens.length) q.lens = [...f.lens]
  if (f.takenFrom) q.taken_from = f.takenFrom
  if (f.takenTo) q.taken_to = f.takenTo
  if (f.tags.length) {
    q.tags = [...f.tags]
    if (f.tagMode === 'all') q.tag_mode = 'all'
  }
  if (f.sharp) q.sharp = f.sharp
  if (f.faceIds.length) {
    q.face_ids = [...f.faceIds]
    if (f.faceSimilarity !== null) q.face_similarity = f.faceSimilarity
  }
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
  const search = normalizeSearch(f.search)
  if (search) q.search = search
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
    f.bestOnly ||
    f.edited ||
    f.duplicates ||
    typeof f.eventId === 'number' ||
    f.device.length > 0 ||
    f.lens.length > 0 ||
    f.takenFrom !== null ||
    f.takenTo !== null ||
    f.tags.length > 0 ||
    f.sharp !== null ||
    f.faceIds.length > 0 ||
    normalizeSearch(f.search) !== '' ||
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

/** Toggle one device (or 'none') in the device filter selection. */
export function toggleDevice(sel: Array<number | 'none'>, id: number | 'none'): Array<number | 'none'> {
  return sel.includes(id) ? sel.filter((x) => x !== id) : [...sel, id]
}

/** Toggle one value (lens name, keyword) in a multi-select. */
export function toggleValue<T>(sel: readonly T[], v: T): T[] {
  return sel.includes(v) ? sel.filter((x) => x !== v) : [...sel, v]
}

const DATE_RE = /^(\d{4})-(\d{2})-(\d{2})$/

/** Is this a real calendar date written `YYYY-MM-DD`? */
export function isIsoDate(v: string | null | undefined): v is string {
  const m = v ? DATE_RE.exec(v) : null
  if (!m) return false
  const [y, mo, d] = [Number(m[1]), Number(m[2]), Number(m[3])]
  const t = new Date(Date.UTC(y, mo - 1, d))
  return t.getUTCFullYear() === y && t.getUTCMonth() === mo - 1 && t.getUTCDate() === d
}

/** `taken_at` (ms, wall clock as UTC) -> `YYYY-MM-DD`. */
export function msToDate(ms: number): string {
  return new Date(ms).toISOString().slice(0, 10)
}

/** Sets one end of the date range; an invalid or inverted entry is rejected (returns the old range). */
export function setDateRange(f: Pick<FilterState, 'takenFrom' | 'takenTo'>, end: 'from' | 'to', value: string): Pick<FilterState, 'takenFrom' | 'takenTo'> {
  const v = value === '' ? null : value
  if (v !== null && !isIsoDate(v)) return { takenFrom: f.takenFrom, takenTo: f.takenTo }
  const next = { takenFrom: end === 'from' ? v : f.takenFrom, takenTo: end === 'to' ? v : f.takenTo }
  // keep from <= to: moving one end past the other drags the other along
  if (next.takenFrom && next.takenTo && next.takenFrom > next.takenTo) {
    if (end === 'from') next.takenTo = next.takenFrom
    else next.takenFrom = next.takenTo
  }
  return next
}

/** Similarity clamped to the slider range. */
export const clampFaceSimilarity = (v: number): number => Math.round(Math.min(FACE_SIMILARITY_MAX, Math.max(FACE_SIMILARITY_MIN, v)) * 100) / 100
