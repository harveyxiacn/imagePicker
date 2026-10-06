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
}

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
export type SortKey = 'taken_at' | '-taken_at' | 'name' | 'rating'

export interface PhotosQuery {
  session_id: number
  rating_gte?: number
  flag?: FlagFilter
  color_label?: ColorName
  sort?: SortKey
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

export interface ApiErrorBody {
  error: { code: string; message: string }
}
