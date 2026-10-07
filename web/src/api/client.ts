import type {
  Device,
  AnalysisProfile,
  AnalysisStatus,
  ApiErrorBody,
  BurstFaces,
  ExportBody,
  FsList,
  GroupsResponse,
  HardwareInfo,
  ImportBody,
  ModelInfo,
  Person,
  Photo,
  PhotoAnalysis,
  PhotoPatchBody,
  PhotosQuery,
  PhotosResponse,
  Session,
  Face,
  AutoMode,
  EditStack,
  EditsPutResponse,
  EditsResponse,
  Preset,
  PreviewBody,
  PreviewResult,
  SyncBody,
  Adjust,
  LutInfo,
  BeautyProfile,
  BestPeopleResponse,
  Collection,
  FaceSearchResponse,
  PhotoPeopleResponse,
  Taste,
  BestTakePlan,
  BestTakeChoice,
  BystandersResponse,
  InpaintBody,
  EnhanceBody,
  AssistantContext,
  AssistantPlan,
  AssistantStatus,
  AuthMe,
  CacheInfo,
  CacheKind,
  LanInfo,
  OnboardingInfo,
  Role,
  RemoteConnectBody,
  RemoteDevice,
  RemotePairStart,
  RemoteStatus,
  RuntimeInfo,
  Settings,
  SettingsPatch,
} from './types'

export class ApiError extends Error {
  status: number
  code: string
  /** Parsed JSON error body (e.g. `models` of a 409 models_missing). */
  body: unknown
  constructor(status: number, code: string, message: string, body?: unknown) {
    super(message)
    this.status = status
    this.code = code
    this.body = body
  }
}

const BASE = '/api'

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(BASE + path, {
    method,
    headers: body !== undefined ? { 'Content-Type': 'application/json' } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  })
  if (!res.ok) throw await toApiError(res, path)
  if (res.status === 204) return undefined as T
  return (await res.json()) as T
}

/** multipart/form-data request (the browser sets the boundary header). */
async function requestForm<T>(method: string, path: string, body: FormData): Promise<T> {
  const res = await fetch(BASE + path, { method, body })
  if (!res.ok) throw await toApiError(res, path)
  return (await res.json()) as T
}

/** Hooks installed by the app shell (kept out of this module so it stays free of router imports). */
export const authHooks: { onUnauthorized: ((requestPath: string) => void) | null } = { onUnauthorized: null }

async function toApiError(res: Response, path = ''): Promise<ApiError> {
  if (res.status === 401) authHooks.onUnauthorized?.(path)
  let code = 'http_error'
  let message = `${res.status} ${res.statusText}`
  let body: unknown
  try {
    const j = (await res.json()) as ApiErrorBody
    body = j
    code = j.error.code
    message = j.error.message
  } catch {
    /* non-JSON error body */
  }
  return new ApiError(res.status, code, message, body)
}

/** POST /api/render/preview: JPEG body plus `X-Render-Ms` / `X-Render-Backend` headers. Abortable. */
export async function renderPreview(body: PreviewBody, signal?: AbortSignal): Promise<PreviewResult> {
  const res = await fetch(`${BASE}/render/preview`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
    signal,
  })
  if (!res.ok) throw await toApiError(res, '/render/preview')
  const ms = Number(res.headers.get('X-Render-Ms'))
  const backend = res.headers.get('X-Render-Backend')
  return {
    blob: await res.blob(),
    renderMs: Number.isFinite(ms) && res.headers.has('X-Render-Ms') ? ms : null,
    backend: backend === 'gpu' || backend === 'cpu' ? backend : null,
  }
}

/** GET /api/masks/{id}: 8-bit grey PNG. 409 models_missing / 503 surface as ApiError. */
export async function fetchMask(photoId: number, target: string, personId?: number, signal?: AbortSignal): Promise<Blob> {
  const q = new URLSearchParams({ target })
  if (personId !== undefined) q.set('person_id', String(personId))
  const res = await fetch(`${BASE}/masks/${photoId}?${q.toString()}`, { signal })
  if (!res.ok) throw await toApiError(res, '/masks')
  return res.blob()
}

/** Builds the query string for GET /api/photos. Omits unset values. */
export function photosQueryString(q: PhotosQuery): string {
  const p = new URLSearchParams()
  p.set('session_id', String(q.session_id))
  if (q.rating_gte !== undefined) p.set('rating_gte', String(q.rating_gte))
  if (q.flag) p.set('flag', q.flag)
  if (q.color_label) p.set('color_label', q.color_label)
  if (q.ai_rating_gte !== undefined) p.set('ai_rating_gte', String(q.ai_rating_gte))
  if (q.issues_none) p.set('issues_none', '1')
  if (q.issues_any?.length) p.set('issues_any', q.issues_any.join(','))
  if (q.burst_best_only) p.set('burst_best_only', '1')
  if (q.burst_id !== undefined) p.set('burst_id', String(q.burst_id))
  if (q.scene_type) p.set('scene_type', q.scene_type)
  if (q.persons?.length) p.set('persons', q.persons.join(','))
  if (q.person_mode) p.set('person_mode', q.person_mode)
  if (q.exclude_persons?.length) p.set('exclude_persons', q.exclude_persons.join(','))
  if (q.person_state?.length) p.set('person_state', q.person_state.join(','))
  if (q.include_background) p.set('include_background', '1')
  if (q.faces_min !== undefined) p.set('faces_min', String(q.faces_min))
  if (q.faces_max !== undefined) p.set('faces_max', String(q.faces_max))
  if (q.device?.length) p.set('device', q.device.join(','))
  if (q.has_edits) p.set('has_edits', '1')
  if (q.sort) p.set('sort', q.sort)
  if (q.cursor) p.set('cursor', q.cursor)
  if (q.limit) p.set('limit', String(q.limit))
  return p.toString()
}

const unwrapSettings = (r: Settings | { settings: Settings }): Settings => ('settings' in r ? r.settings : r)

export const api = {
  health: () => request<{ ok: boolean; version: string }>('GET', '/health'),
  importFolder: (body: ImportBody) => request<{ session: Session }>('POST', '/import', body),
  sessions: () => request<{ sessions: Session[] }>('GET', '/sessions'),
  session: (id: number) => request<{ session: Session }>('GET', `/sessions/${id}`),
  deleteSession: (id: number) => request<void>('DELETE', `/sessions/${id}`),
  photos: (q: PhotosQuery) => request<PhotosResponse>('GET', `/photos?${photosQueryString(q)}`),
  photo: (id: number) => request<{ photo: Photo }>('GET', `/photos/${id}`),
  patchPhotos: (body: PhotoPatchBody) => request<{ updated: number }>('PATCH', '/photos', body),
  viewport: (ids: number[]) => request<void>('POST', '/viewport', { ids }),
  exportPhotos: (body: ExportBody) => request<{ task_id: string }>('POST', '/export', body),
  // ---- M7: AI runtime ----
  runtime: () => request<RuntimeInfo>('GET', '/runtime'),
  installRuntime: (extras?: string[]) => request<{ task_id: string }>('POST', '/runtime/install', extras ? { extras } : {}),
  cancelRuntime: () => request<void>('POST', '/runtime/cancel'),
  removeRuntime: () => request<void>('DELETE', '/runtime'),
  // ---- M2 ----
  hardware: () => request<HardwareInfo>('GET', '/system/hardware'),
  models: () => request<{ models: ModelInfo[] }>('GET', '/models'),
  ensureModels: (ids: string[]) => request<{ task_id: string }>('POST', '/models/ensure', { ids }),
  runAnalysis: (body: { session_id: number; profile: AnalysisProfile; photo_ids?: number[] }) =>
    request<{ task_id: string }>('POST', '/analysis/run', body),
  cancelAnalysis: (session_id: number) => request<void>('POST', '/analysis/cancel', { session_id }),
  analysisStatus: (session_id: number) => request<AnalysisStatus>('GET', `/analysis/status?session_id=${session_id}`),
  photoAnalysis: (id: number) => request<PhotoAnalysis>('GET', `/photos/${id}/analysis`),
  groups: (session_id: number) => request<GroupsResponse>('GET', `/groups?session_id=${session_id}`),
  splitGroup: (burst_id: number, at_photo_id: number) =>
    request<{ burst_ids: number[] }>('POST', '/groups/split', { burst_id, at_photo_id }),
  mergeGroups: (burst_ids: number[]) => request<{ burst_id: number }>('POST', '/groups/merge', { burst_ids }),
  burstFaces: (id: number) => request<BurstFaces>('GET', `/bursts/${id}/faces`),
  acceptAi: (ids: number[]) => request<{ updated: number }>('POST', '/photos/accept-ai', { ids }),
  devices: (sessionId: number) => request<{ devices: Device[] }>('GET', `/sessions/${sessionId}/devices`),
  people: (session_id?: number) =>
    request<{ people: Person[] }>('GET', `/people${session_id !== undefined ? `?session_id=${session_id}` : ''}`),
  patchPerson: (id: number, body: { name?: string | null; hidden?: boolean }) =>
    request<{ person: Person }>('PATCH', `/people/${id}`, body),
  mergePeople: (ids: number[], into: number) => request<{ person: Person }>('POST', '/people/merge', { ids, into }),
  setFacePerson: (id: number, person_id: number | null) =>
    request<{ face: Face }>('POST', `/faces/${id}/person`, { person_id }),
  // ---- M3 ----
  edits: (id: number) => request<EditsResponse>('GET', `/edits/${id}`),
  putEdits: (id: number, stack: EditStack) => request<EditsPutResponse>('PUT', `/edits/${id}`, { stack }),
  deleteEdits: (id: number) => request<void>('DELETE', `/edits/${id}`),
  autoEdit: (id: number, mode: AutoMode) => request<{ adjust: Adjust }>('POST', `/edits/${id}/auto`, { mode }),
  syncEdits: (body: SyncBody) => request<{ updated: number }>('POST', '/edits/sync', body),
  presets: () => request<{ presets: Preset[] }>('GET', '/presets'),
  savePreset: (name: string, stack: EditStack) => request<{ preset: Preset }>('POST', '/presets', { name, stack }),
  deletePreset: (id: string) => request<void>('DELETE', `/presets/${encodeURIComponent(id)}`),
  luts: () => request<{ luts: LutInfo[] }>('GET', '/luts'),
  importLut: (path: string) => request<{ id: string; name: string }>('POST', '/luts/import', { path }),
  // ---- M4 ----
  photoPeople: (id: number) => request<PhotoPeopleResponse>('GET', `/photos/${id}/people`),
  beautyPrepare: (id: number) => request<{ task_id: string }>('POST', `/photos/${id}/beauty/prepare`),
  beautyProfile: (personId: number) => request<{ profile: BeautyProfile | null }>('GET', `/people/${personId}/beauty-profile`),
  putBeautyProfile: (personId: number, profile: BeautyProfile | null) =>
    request<{ profile: BeautyProfile | null }>('PUT', `/people/${personId}/beauty-profile`, { profile }),
  applyProfiles: (photo_ids: number[]) => request<{ updated: number }>('POST', '/edits/apply-profiles', { photo_ids }),
  bestPeople: (session_id: number, ids: number[], n: number) =>
    request<BestPeopleResponse>('GET', `/people/best?session_id=${session_id}&ids=${ids.join(',')}&n=${n}`),
  faceSearch: (input: { image: File } | { face_id: number }, opts: { session_id?: number; face_index?: number } = {}) => {
    if ('image' in input) {
      const fd = new FormData()
      fd.append('image', input.image)
      if (opts.session_id !== undefined) fd.append('session_id', String(opts.session_id))
      if (opts.face_index !== undefined) fd.append('face_index', String(opts.face_index))
      return requestForm<FaceSearchResponse>('POST', '/faces/search', fd)
    }
    return request<FaceSearchResponse>('POST', '/faces/search', { ...input, ...opts })
  },
  collections: () => request<{ collections: Collection[] }>('GET', '/collections'),
  createCollection: (name: string, query: string) => request<{ collection: Collection }>('POST', '/collections', { name, query }),
  patchCollection: (id: string, body: { name?: string; query?: string }) =>
    request<{ collection: Collection }>('PATCH', `/collections/${encodeURIComponent(id)}`, body),
  deleteCollection: (id: string) => request<void>('DELETE', `/collections/${encodeURIComponent(id)}`),
  taste: () => request<Taste>('GET', '/taste'),
  resetTaste: () => request<void>('POST', '/taste/reset'),
  // ---- M5 ----
  bestTakePlan: (burstId: number) => request<BestTakePlan>('GET', `/bursts/${burstId}/besttake`),
  bestTake: (base_photo_id: number, choices: BestTakeChoice[]) => request<{ task_id: string }>('POST', '/besttake', { base_photo_id, choices }),
  bestTakeAuto: (burstId: number) => request<{ task_id: string }>('POST', `/bursts/${burstId}/besttake/auto`),
  bystanders: (id: number) => request<BystandersResponse>('GET', `/photos/${id}/bystanders`),
  inpaint: (id: number, body: InpaintBody) => request<{ task_id: string }>('POST', `/photos/${id}/inpaint`, body),
  enhance: (id: number, body: EnhanceBody) => request<{ task_id: string }>('POST', `/photos/${id}/enhance`, body),
  // ---- M6 ----
  assistantStatus: () => request<AssistantStatus>('GET', '/assistant/status'),
  assistantPlan: (body: { session_id: number; message: string; context: AssistantContext; engine?: 'auto' | 'rules' | 'llm' }) =>
    request<AssistantPlan>('POST', '/assistant/plan', body),
  assistantExecute: (plan_id: string) => request<{ task_id: string }>('POST', '/assistant/execute', { plan_id }),
  assistantDescribe: (photo_id: number) => request<{ caption: string; keywords: string[] }>('POST', '/assistant/describe', { photo_id }),
  settings: () => request<Settings | { settings: Settings }>('GET', '/settings').then(unwrapSettings),
  patchSettings: (patch: SettingsPatch) => request<Settings | { settings: Settings }>('PATCH', '/settings', patch).then(unwrapSettings),
  cache: () => request<CacheInfo>('GET', '/cache'),
  clearCache: (kinds: CacheKind[]) => request<void>('POST', '/cache/clear', { kinds }),
  deleteModel: (id: string) => request<void>('DELETE', `/models/${encodeURIComponent(id)}`),
  deleteAllFaces: () => request<void>('DELETE', '/faces?confirm=true'),
  onboarding: () => request<OnboardingInfo>('GET', '/onboarding'),
  onboardingDone: () => request<void>('POST', '/onboarding/done'),
  lan: () => request<LanInfo>('GET', '/system/lan'),
  me: () => request<AuthMe>('GET', '/auth/me'),
  login: (password: string) => request<{ role?: Role }>('POST', '/auth/login', { password }),
  logout: () => request<void>('POST', '/auth/logout'),
  setPassword: (password: string, guest_password?: string) =>
    request<{ ok: boolean }>('POST', '/auth/password', guest_password === undefined ? { password } : { password, guest_password }),
  xmpSync: (session_id: number, direction: 'read' | 'write') => request<{ updated?: number }>('POST', '/xmp/sync', { session_id, direction }),
  // remote AI (M8): phone side
  remoteStatus: () => request<RemoteStatus>('GET', '/remote/status'),
  remoteConnect: (body: RemoteConnectBody) => request<RemoteStatus>('POST', '/remote/connect', body),
  remoteDisconnect: () => request<void>('DELETE', '/remote/connect'),
  // remote AI (M8): host side
  remotePairStart: () => request<RemotePairStart>('POST', '/remote/pair/start'),
  remoteDevices: () => request<{ devices: RemoteDevice[] }>('GET', '/remote/devices'),
  remoteRevoke: (id: string) => request<void>('DELETE', `/remote/devices/${encodeURIComponent(id)}`),
  fsRoots: () => request<{ roots: string[] }>('GET', '/fs/roots'),
  fsList: (path?: string) =>
    request<FsList>('GET', `/fs/list${path ? `?path=${encodeURIComponent(path)}` : ''}`),
}

/** Page size used when pulling a whole session (contract: max 5000). */
export const PAGE_SIZE = 5000

/** Pulls every page of a photos query (client virtualizes up to ~20k items). */
export async function fetchAllPhotos(
  q: Omit<PhotosQuery, 'cursor' | 'limit'>,
): Promise<{ photos: Photo[]; total: number }> {
  const photos: Photo[] = []
  let cursor: string | undefined
  for (;;) {
    const page = await api.photos({ ...q, cursor, limit: PAGE_SIZE })
    photos.push(...page.photos)
    if (!page.next_cursor) return { photos, total: page.total }
    cursor = page.next_cursor
  }
}

// ---- Binary URLs ----

export const thumbUrl = (p: Pick<Photo, 'id' | 'thumb_version'>, s: 256 | 512 = 256) =>
  `${BASE}/thumb/${p.id}?s=${s}&v=${encodeURIComponent(p.thumb_version)}`
/** `v` (thumb_version) busts the cache when edits change the rendered preview. */
export const previewUrl = (id: number, s: 1024 | 2048 | 4096 = 2048, v?: string) =>
  `${BASE}/preview/${id}?s=${s}${v ? `&v=${encodeURIComponent(v)}` : ''}`
export const faceCropUrl = (faceId: number, s: 128 | 256 = 128) => `${BASE}/faces/${faceId}/crop?s=${s}`
/** RGBA PNG of a patch asset (contract M5 C). */
export const assetUrl = (photoId: number, asset: string) => `${BASE}/assets/${photoId}/${encodeURIComponent(asset)}`
export const originalUrl = (id: number) => `${BASE}/original/${id}`
