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
  ids: number[]
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
  | { type: 'worker.status'; state: WorkerState; tier: 'T0' | 'T1' | 'T2' | 'T3' | null; error: string | null }

export interface ApiErrorBody {
  error: { code: string; message: string }
}
