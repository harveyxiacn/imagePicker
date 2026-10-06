// Hand-written from docs/api-contract-m1.md. Keep in sync with the contract.

export type Flag = -1 | 0 | 1
export type ColorLabel = 'red' | 'yellow' | 'green' | 'blue' | 'purple' | null
export type ColorName = Exclude<ColorLabel, null>
export type ImageFormat = 'jpeg' | 'png' | 'webp' | 'heif' | 'avif' | 'tiff' | 'raw'

export interface Photo {
  id: number
  session_id: number
  path: string
  file_name: string
  format: ImageFormat
  file_size: number
  width: number | null
  height: number | null
  taken_at: number | null
  /** UTC offset at capture (minutes); null = unknown, taken_at is then naive wall-clock as UTC */
  taken_at_offset_min: number | null
  camera: string | null
  lens: string | null
  focal_mm: number | null
  aperture: number | null
  shutter_s: number | null
  iso: number | null
  user_rating: number | null
  ai_rating: number | null
  flag: Flag
  color_label: ColorLabel
  burst_id: number | null
  thumb_ready: boolean
  thumb_version: string
  // ---- M2 (docs/api-contract-m2.md C.1) ----
  /** 0-1 composite score; null = not analysed */
  ai_score: number | null
  /** [] = no issues or not analysed */
  issues: Issue[]
  /** 0 = best shot of its burst */
  rank_in_burst: number | null
  burst_size: number | null
  scene_type: SceneType | null
  face_count: number | null
  subject_face_count: number | null
  analyzed: boolean
  // ---- M3 (docs/api-contract-m3.md B) ----
  /** true when a non-empty edit stack is saved; thumbs/previews are then rendered */
  has_edits: boolean
}

export type Issue = 'closed_eyes' | 'blurry' | 'overexposed' | 'underexposed' | 'noisy' | 'tilted'
export type SceneType = 'portrait' | 'group' | 'landscape' | 'food' | 'architecture' | 'night' | 'pet' | 'other'
export const ISSUES: Issue[] = ['closed_eyes', 'blurry', 'overexposed', 'underexposed', 'noisy', 'tilted']
export const SCENE_TYPES: SceneType[] = ['portrait', 'group', 'landscape', 'food', 'architecture', 'night', 'pet', 'other']

export type ImportState = 'scanning' | 'thumbnailing' | 'ready'

export interface Session {
  id: number
  title: string
  root_path: string
  created_at: number
  photo_count: number
  picked_count: number
  rejected_count: number
  rated_count: number
  cover_photo_id: number | null
  import_state: ImportState
}

export type FlagFilter = 'picked' | 'rejected' | 'unflagged' | 'not_rejected'
export type SortKey = 'taken_at' | '-taken_at' | 'name' | 'rating' | 'ai'
export type PersonState = 'eyes_open' | 'smiling' | 'looking' | 'subject'
export type PersonMode = 'all' | 'any'

export interface PhotosQuery {
  session_id: number
  rating_gte?: number
  flag?: FlagFilter
  color_label?: ColorName
  sort?: SortKey
  ai_rating_gte?: number
  issues_none?: boolean
  issues_any?: Issue[]
  burst_best_only?: boolean
  burst_id?: number
  scene_type?: SceneType
  persons?: number[]
  person_mode?: PersonMode
  exclude_persons?: number[]
  person_state?: PersonState[]
  include_background?: boolean
  faces_min?: number
  faces_max?: number
  /** M4: only photos with a saved non-empty edit stack (built-in "edited" collection) */
  has_edits?: boolean
  cursor?: string
  limit?: number
}

export interface PhotosResponse {
  photos: Photo[]
  total: number
  next_cursor: string | null
}

export interface PhotoPatchBody {
  ids: number[]
  user_rating?: number | null
  flag?: Flag
  color_label?: ColorLabel
}

/** Editable fields of a photo (what PATCH /api/photos accepts). */
export interface PhotoEditable {
  user_rating: number | null
  flag: Flag
  color_label: ColorLabel
}

export interface ImportBody {
  path: string
  recursive?: boolean
  title?: string
}

export interface ExportBody {
  /** M5: super-resolution factor applied after rendering (slow on CPU tiers) */
  upscale?: 2 | 4
  /** exactly one of `ids` / `folders` (M4 C.2) */
  ids?: number[]
  /** sub-folder name -> photo ids (export by person) */
  folders?: Record<string, number[]>
  dest: string
  long_edge?: number | null
  quality?: number
  name_template?: string
}

export interface FsList {
  path: string
  parent: string | null
  dirs: string[]
  image_count: number
}

// ---- WebSocket events (server -> client) ----

export type TaskState = 'running' | 'done' | 'failed'

export interface PhotoUpdateItem {
  id: number
  user_rating?: number | null
  flag?: Flag
  color_label?: ColorLabel
}

export type WorkerState = 'stopped' | 'starting' | 'ready' | 'busy' | 'crashed' | 'unavailable'
export type AnalysisProfile = 'fast' | 'standard'
export type AnalysisStage = 'analyzing' | 'grouping' | 'scoring' | 'clustering'

export interface HardwareInfo {
  worker: {
    state: WorkerState
    tier: 'T0' | 'T1' | 'T2' | 'T3' | null
    device: string | null
    providers: string[]
    gpu: { name: string; vram_mb: number } | null
    error: string | null
  }
}

export interface ModelInfo {
  id: string
  task: string[]
  size_mb: number
  license: string
  noncommercial: boolean
  installed: boolean
  required_for: AnalysisProfile[]
}

export interface AnalysisStatus {
  state: 'idle' | 'running' | 'done' | 'failed'
  profile: AnalysisProfile | null
  done: number
  total: number
  stage: AnalysisStage | null
  error: string | null
}

export interface Face {
  id: number
  photo_id: number
  person_id: number | null
  person_name: string | null
  /** normalized x, y, w, h (display orientation) */
  bbox: [number, number, number, number]
  eyes_open: number | null
  smile: number | null
  gaze: number | null
  yaw: number | null
  pitch: number | null
  roll: number | null
  sharpness: number | null
  expression_score: number | null
  is_subject: boolean
}

export interface PhotoAnalysis {
  photo_id: number
  analyzed: boolean
  profile: AnalysisProfile | null
  scores: {
    sharpness: number | null
    exposure: number | null
    noise: number | null
    iqa: number | null
    aesthetic: number | null
    face: number | null
    composition: number | null
  }
  ai_score: number | null
  ai_rating: number | null
  contributions: { key: string; label_key: string; delta: number }[]
  reasons: { key: string; params?: Record<string, string | number> }[]
  scene_type: SceneType | null
  faces: Face[]
}

export interface Burst {
  id: number
  best_photo_id: number
  /** sorted by rank (best first) */
  photo_ids: number[]
  size: number
  start_at: number
}
export interface Scene {
  id: number
  start_at: number
  end_at: number
  bursts: Burst[]
}
export interface GroupsResponse {
  scenes: Scene[]
}

export interface FaceTrack {
  track_id: number
  person_id: number | null
  person_name: string | null
  cells: Record<string, Face | null>
  /** top-3 expression frames for this person */
  best_photo_ids: number[]
}
export interface BurstFaces {
  photo_ids: number[]
  tracks: FaceTrack[]
}

export interface Person {
  id: number
  name: string | null
  cover_face_id: number
  photo_count: number
  hidden: boolean
}

export type ServerEvent =
  | { type: 'session.updated'; session: Session }
  | { type: 'photos.added'; session_id: number; count: number }
  | { type: 'thumbs.ready'; items: { id: number; v: string }[] }
  | { type: 'photos.updated'; items: PhotoUpdateItem[] }
  | {
      type: 'task.progress'
      task_id: string
      kind: string
      done: number
      total: number
      state: TaskState
      error?: string
    }
  | {
      type: 'analysis.progress'
      session_id: number
      state: AnalysisStatus['state']
      stage: AnalysisStage | null
      done: number
      total: number
    }
  | { type: 'analysis.updated'; session_id: number; ids: number[] }
  | { type: 'groups.updated'; session_id: number }
  | { type: 'people.updated'; session_id: number }
  | { type: 'edits.updated'; items: { id: number; has_edits: boolean; thumb_version: string }[] }
  | { type: 'worker.status'; state: WorkerState; tier: 'T0' | 'T1' | 'T2' | 'T3' | null; error: string | null }
  | { type: 'beauty.ready'; photo_id: number }
  | { type: 'besttake.done'; photo_id: number; results: BestTakeResult[] }
  | { type: 'inpaint.done'; photo_id: number; ok: boolean; reason: string | null }
  | { type: 'enhance.done'; photo_id: number; op: EnhanceOp; ok: boolean; reason: string | null }
  | { type: 'taste.updated'; labels: number; active: boolean; alpha: number }
  | { type: 'collections.updated' }
  | { type: 'assistant.done'; plan_id: string; ok: boolean; results: AssistantResult[]; undo: AssistantUndo }
  | { type: 'settings.updated'; settings: Settings }
  | { type: 'runtime.updated'; runtime: RuntimeInfo }
  | { type: 'xmp.conflict'; photo_id: number; sidecar: { rating?: number | null }; catalog: { rating?: number | null } }

// ---- M7: installable AI runtime (GET /api/runtime) ----
export type RuntimeState = 'missing' | 'installing' | 'ready' | 'failed'

export interface RuntimeInfo {
  state: RuntimeState
  /** installed runtime version (null when none) */
  version: string | null
  bundled_version: string
  /** an app update left an older runtime behind: it must be re-installed */
  outdated: boolean
  /** installed extras, e.g. ["cuda","mediapipe"] */
  extras: string[]
  /** extras the installer would pick for this machine */
  recommended_extras?: string[]
  python: string | null
  venv_bytes: number | null
  error: string | null
  can_install: boolean
  /** why `can_install` is false */
  reason: string | null
  /** running step: prepare | python | sync | verify */
  step: string | null
  /** 0..100 while installing */
  percent: number
  detail: string
  task_id: string | null
  hardware?: { os: string; arch: string; nvidia: { name: string; driver: string } | null }
}

export interface ApiErrorBody {
  error: { code: string; message: string }
}

// ---- M3: edit stack (mirrors crates/ip-render/src/lib.rs; serde JSON shape) ----

export type Point = [number, number]

export interface CropOp {
  type: 'crop'
  /** normalised [x, y, w, h] of the kept region, measured after `angle` rotation */
  rect: [number, number, number, number]
  /** straighten angle in degrees, positive = counter-clockwise, |angle| <= 45 */
  angle: number
  /** UI hint only (e.g. "4:5") */
  aspect?: string
}

export const HSL_BANDS = ['red', 'orange', 'yellow', 'green', 'aqua', 'blue', 'purple', 'magenta'] as const
export type HslBand = (typeof HSL_BANDS)[number]

export interface Hsl {
  h: number
  s: number
  l: number
}

export interface Curves {
  rgb?: Point[]
  r?: Point[]
  g?: Point[]
  b?: Point[]
}
export type CurveChannel = keyof Curves

/** Three-way colour grading; each wheel is [hue_degrees 0..360, amount 0..1]. */
export interface Grading {
  shadows: Point
  midtones: Point
  highlights: Point
  /** -100 (favour shadows) .. 100 (favour highlights) */
  balance: number
}

/**
 * Slider-style adjustments. Rust serialises every numeric field; on the wire (PUT) any field may be
 * omitted and defaults to 0 / neutral (`#[serde(default)]`), so all are optional here.
 */
export interface Adjust {
  /** EV stops, -5..5 */
  exposure?: number
  contrast?: number
  highlights?: number
  shadows?: number
  whites?: number
  blacks?: number
  /** Kelvin shift relative to as-shot, -3000..3000 (positive = warmer) */
  temp?: number
  tint?: number
  vibrance?: number
  saturation?: number
  clarity?: number
  dehaze?: number
  curve?: Curves
  hsl?: Partial<Record<HslBand, Partial<Hsl>>>
  grading?: Partial<Grading>
  /** provenance, e.g. "ai_auto@1" or "user"; not used for rendering */
  source?: string
}

export const ADJUST_NUMERIC_KEYS = [
  'exposure',
  'contrast',
  'highlights',
  'shadows',
  'whites',
  'blacks',
  'temp',
  'tint',
  'vibrance',
  'saturation',
  'clarity',
  'dehaze',
] as const
export type AdjustKey = (typeof ADJUST_NUMERIC_KEYS)[number]

export type MaskTarget = 'subject' | 'background' | 'sky' | 'person' | 'skin' | 'hair' | 'clothes'
export const MASK_TARGETS: MaskTarget[] = ['subject', 'background', 'sky', 'person', 'skin', 'hair', 'clothes']

export type MaskRef =
  | { kind: 'ai'; target: MaskTarget; person_id?: number }
  | { kind: 'radial'; center: Point; radius: Point; feather?: number }
  | { kind: 'linear'; start: Point; end: Point }

export interface GlobalOp extends Adjust {
  type: 'global'
}
export interface LocalOp {
  type: 'local'
  mask: MaskRef
  /** mask opacity 0..1 (default 1) */
  amount?: number
  invert?: boolean
  adjust: Adjust
}
export interface LutOp {
  type: 'lut'
  file: string
  amount?: number
}
export interface OutputSharpenOp {
  type: 'output_sharpen'
  amount: number
}
// ---- M4: portrait ops (mirrors ip-render `Level`, `Beauty`, `Warp::{Face,Body}`) ----

export type Level = 'natural' | 'standard' | 'refined'
export const LEVELS: Level[] = ['natural', 'standard', 'refined']

/** Skin retouching; every amount 0..100 (0 = off). `person_id` omitted/null = every detected face. */
export interface BeautyOp {
  type: 'beauty'
  person_id?: number | null
  level: Level
  smooth: number
  whiten: number
  blemish: boolean
  eye_brighten: number
  teeth_whiten: number
  dark_circles: number
}
/** Parametric face liquify, amounts -100..100. */
export interface WarpFaceOp {
  type: 'warp'
  kind: 'face'
  person_id?: number | null
  level: Level
  slim: number
  chin: number
  eyes: number
  nose: number
}
/** Parametric body liquify, amounts 0..100. */
export interface WarpBodyOp {
  type: 'warp'
  kind: 'body'
  person_id?: number | null
  level: Level
  arms: number
  legs: number
  waist: number
  lengthen_legs: number
  protect_background: boolean
}
export type WarpOp = WarpFaceOp | WarpBodyOp
export type PortraitOp = BeautyOp | WarpOp

export const BEAUTY_KEYS = ['smooth', 'whiten', 'eye_brighten', 'teeth_whiten', 'dark_circles'] as const
export type BeautyKey = (typeof BEAUTY_KEYS)[number]
export const WARP_FACE_KEYS = ['slim', 'chin', 'eyes', 'nose'] as const
export type WarpFaceKey = (typeof WARP_FACE_KEYS)[number]
export const WARP_BODY_KEYS = ['arms', 'legs', 'waist', 'lengthen_legs'] as const
export type WarpBodyKey = (typeof WARP_BODY_KEYS)[number]

/** Saved per-person look (docs/api-contract-m4.md C.1); applied by filling in `person_id`. */
export interface BeautyProfile {
  beauty?: Omit<BeautyOp, 'type' | 'person_id'>
  face?: Omit<WarpFaceOp, 'type' | 'kind' | 'person_id'>
  body?: Omit<WarpBodyOp, 'type' | 'kind' | 'person_id'>
}

export interface PhotoPerson {
  face_id: number
  person_id: number | null
  person_name: string | null
  /** normalised x, y, w, h (upright image) */
  face_box: [number, number, number, number]
  is_subject: boolean
  has_pose: boolean
  has_profile: boolean
}
export interface PhotoPeopleResponse {
  people: PhotoPerson[]
  ready: boolean
}

// ---- M4: M2 remainder ----

export interface BestPeopleResponse {
  people: { person_id: number; photos: { photo_id: number; score: number }[] }[]
}

export interface FaceSearchCandidate {
  person_id: number
  person_name: string | null
  similarity: number
}
export interface FaceSearchResponse {
  /** normalised [x, y, w, h] of every face found in the query image */
  faces_detected: [number, number, number, number][]
  /** index into `faces_detected` the candidates belong to */
  query_face: number
  candidates: FaceSearchCandidate[]
  similar_faces: { face_id: number; photo_id: number; similarity: number }[]
}

export interface Collection {
  id: string
  name: string
  /** URLSearchParams string with the /api/photos filter params (no session_id / cursor / limit) */
  query: string
  builtin: boolean
}

export interface TasteTrait {
  key: string
  params?: Record<string, string | number>
}
export interface Taste {
  labels: number
  active: boolean
  /** 0..0.6 fusion weight */
  alpha: number
  holdout_accuracy: number | null
  traits: TasteTrait[]
  updated_at: number | null
}

// ---- M5: patch layers, best take, inpaint, enhance (docs/api-contract-m5.md; mirrors ip-render `Patch`) ----

export type PatchKind = 'best_take' | 'inpaint' | 'denoise' | 'face_restore'
export const PATCH_KINDS: PatchKind[] = ['best_take', 'inpaint', 'denoise', 'face_restore']

/** A generated raster layer: RGBA PNG `asset` (alpha = blend mask) placed over `rect` of the upright, pre-crop image. */
export interface PatchOp {
  type: 'patch'
  kind: PatchKind
  asset: string
  /** normalised [x, y, w, h], upright image, before crop */
  rect: [number, number, number, number]
  /** extra edge feather, fraction of the rect's short side, 0..0.5 */
  feather?: number
  /** blend opacity 0..1 */
  amount?: number
  enabled?: boolean
  /** best take: whose face was replaced and from which photo */
  person_id?: number
  source_photo_id?: number
}

export type BestTakeWarning = 'large_pose_change' | 'camera_moved' | 'occlusion' | 'seam'

export interface BestTakeCandidate {
  photo_id: number
  face_id: number
  expression_score: number
  composable: boolean
  reason: string | null
}
export interface BestTakePerson {
  track_id: number
  person_id: number | null
  person_name: string | null
  base_face_id: number
  /** sorted by expression score, best first */
  candidates: BestTakeCandidate[]
  best_photo_id: number
}
export interface BestTakePlan {
  base_photo_id: number
  people: BestTakePerson[]
}
export interface BestTakeChoice {
  base_face_id: number
  source_photo_id: number
  source_face_id: number
}
export interface BestTakeResult {
  base_face_id: number
  ok: boolean
  warnings: BestTakeWarning[]
  reason: string | null
}

export interface BystandersResponse {
  faces: { face_id: number; bbox: [number, number, number, number] }[]
}

/** Brush stroke: normalised points (upright, pre-crop); `radius` is a fraction of the image WIDTH. */
export interface Stroke {
  points: Point[]
  radius: number
}
export type InpaintBody =
  | { bystanders: true; model?: 'lama' | 'sdxl' }
  | { face_ids: number[]; model?: 'lama' | 'sdxl' }
  | { strokes: Stroke[]; model?: 'lama' | 'sdxl' }

export type EnhanceOp = 'denoise' | 'face_restore'
export interface EnhanceBody {
  op: EnhanceOp
  strength: number
}

/** Other ops are preserved verbatim and ignored by the UI. */
export interface UnknownOp {
  type: string
  [k: string]: unknown
}
export type KnownOp = CropOp | GlobalOp | LocalOp | LutOp | OutputSharpenOp
export type Op = KnownOp | PortraitOp | PatchOp | UnknownOp

export interface EditStack {
  version: number
  ops: Op[]
}

export type EditSection = 'crop' | 'global' | 'local' | 'lut' | 'output_sharpen'
export const EDIT_SECTIONS: EditSection[] = ['crop', 'global', 'local', 'lut', 'output_sharpen']

export interface EditsResponse {
  photo_id: number
  stack: EditStack
  updated_at: number | null
}
export interface EditsPutResponse {
  photo_id: number
  stack: EditStack
  updated_at: number
  thumb_version: string
}

export type AutoMode = 'auto' | 'portrait' | 'landscape'

export interface SyncBody {
  from_id: number
  to_ids: number[]
  include: EditSection[]
  adaptive?: boolean
}

export interface Preset {
  id: string
  /** built-in presets carry an i18n key such as `preset.film_warm` */
  name: string
  builtin: boolean
  stack: EditStack
}

export interface PreviewBody {
  photo_id: number
  stack?: EditStack
  long_edge?: number
  original?: boolean
}
export interface PreviewResult {
  blob: Blob
  renderMs: number | null
  backend: 'gpu' | 'cpu' | null
}

/** `GET /api/luts`: built-ins (`name` is an i18n key `lut.<id>`) and imported `.cube` files. */
export interface LutInfo {
  id: string
  name: string
  builtin: boolean
}

// ---- M6 (docs/api-contract-m6.md) ----

export type AssistantTool =
  | 'filter'
  | 'set_rating'
  | 'set_flag'
  | 'accept_ai'
  | 'group_keep_top'
  | 'scene_keep_top'
  | 'apply_preset'
  | 'auto_adjust'
  | 'apply_profiles'
  | 'besttake_auto'
  | 'remove_bystanders'
  | 'export'
  | 'describe'
  | 'suggest_edits'

export type AssistantEngine = 'rules' | 'llm'

export interface AssistantStatus {
  engine: AssistantEngine
  llm_model: string | null
  vlm_model: string | null
  llm_available: boolean
  vlm_available: boolean
}

export interface AssistantContext {
  /** current filter as a /api/photos query string (no session_id) */
  filter: string
  selection: number[]
  current_photo_id: number | null
  locale: 'zh-CN' | 'en'
}

export interface PlanStep {
  tool: AssistantTool | (string & {})
  args: Record<string, unknown>
  summary: string
  affects: number
  destructive: boolean
}

export interface AssistantPlan {
  plan_id: string
  reply: string
  steps: PlanStep[]
  needs_confirmation: boolean
  engine: AssistantEngine
  unsupported: string | null
}

export interface AssistantResult {
  tool: string
  ok: boolean
  affected: number
  error?: string | null
  /** `suggest_edits` / `describe` payloads (client-side extension, see docs of the web UI) */
  data?: unknown
}

/** Values BEFORE the execution: enough to revert it as one undo step. */
export interface AssistantUndo {
  edits: { photo_id: number; before: EditStack }[]
  photos: { id: number; user_rating: number | null; flag: Flag; color_label: ColorLabel }[]
}

export interface EditSuggestion {
  problems: string[]
  adjust: Adjust
  reason: string
}

export type XmpMode = 'off' | 'sidecar' | 'sidecar_and_embedded'
export type ModelSource = 'auto' | 'hf' | 'hf-mirror' | 'modelscope'
export type GroupStrictness = 'loose' | 'normal' | 'strict'
export type RenderBackend = 'auto' | 'gpu' | 'cpu'

export interface Settings {
  language: 'zh-CN' | 'en'
  theme: 'dark' | 'light' | 'system'
  analysis: { default_profile: AnalysisProfile; auto_analyze_on_import: boolean; group_strictness: GroupStrictness }
  faces: { enabled: boolean }
  privacy: { allow_network: boolean }
  models: { dir: string; source: ModelSource }
  cache: { max_gb: number }
  render: { backend: RenderBackend }
  xmp_mode: XmpMode
  lan: { enabled: boolean; port: number; guest_enabled: boolean }
  roots: string[]
  assistant: { engine: 'auto' | 'rules' | 'llm' }
}

/** Recursive partial used for `PATCH /api/settings`. */
export type SettingsPatch = {
  [K in keyof Settings]?: Settings[K] extends unknown[] ? Settings[K] : Settings[K] extends object ? Partial<Settings[K]> : Settings[K]
}

export type CacheKind = 'thumbs' | 'previews' | 'masks' | 'edits' | 'gen'
export const CACHE_KINDS: CacheKind[] = ['thumbs', 'previews', 'masks', 'edits', 'gen']
export interface CacheInfo {
  bytes: number
  items: Record<CacheKind, number>
}

export interface OnboardingInfo {
  first_run: boolean
  hardware: HardwareInfo['worker']
  recommended_tier: 'T0' | 'T1' | 'T2' | 'T3'
  recommended_download_mb: number
  /** ids of the models to download for the recommended tier (optional extension; falls back to the analysis models) */
  recommended_models?: string[]
}

export type Role = 'owner' | 'guest'
export interface AuthMe {
  role: Role | null
  lan: boolean
}

export interface LanInfo {
  enabled: boolean
  urls: string[]
  qr_svg: string | null
  restart_required?: boolean
}
