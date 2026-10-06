/**
 * Android branch of the platform adapter: thin typed wrappers around the Kotlin plugin
 * `imagepicker-media` (docs/api-contract-m8.md section A). Only available inside the Android
 * Tauri shell; everywhere else `getMedia()` returns null.
 */

type Invoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>

export interface PermissionState {
  /** Some media access is available (full or partial). */
  granted: boolean
  /** Android 14 "Selected photos". */
  partial: boolean
}

export interface Album {
  id: string
  name: string
  /** Directory, e.g. /storage/emulated/0/DCIM/Camera - pass this to the import API. */
  path: string
  count: number
  cover_path: string
  latest_ms: number
}

export interface MediaApi {
  requestPermission: () => Promise<PermissionState>
  listAlbums: () => Promise<Album[]>
  share: (paths: string[]) => Promise<number>
  /** Moves photos to the system trash after the user confirms; resolves with the count. */
  trash: (paths: string[]) => Promise<number>
  /** Start (or update) the analysis foreground service. progress 0..100, omit = indeterminate. */
  startForeground: (title: string, text: string, progress?: number) => Promise<void>
  stopForeground: () => Promise<void>
  keepAwake: (on: boolean) => Promise<void>
  /** Copies exported files (app-private) into Pictures/imagePicker/ via MediaStore. */
  publishExports: (paths: string[]) => Promise<{ published: number; uris: string[] }>
}

export const isAndroidShell = (): boolean =>
  typeof window !== 'undefined' && !!window.__TAURI_INTERNALS__ && /Android/i.test(navigator.userAgent)

const P = 'plugin:imagepicker-media|'

export function createMediaApi(invoke: Invoke): MediaApi {
  const call = <T>(cmd: string, args?: Record<string, unknown>) => invoke(P + cmd, args) as Promise<T>
  return {
    requestPermission: () => call<PermissionState>('request_permission'),
    listAlbums: async () => (await call<{ albums: Album[] }>('list_albums')).albums,
    share: async (paths) => (await call<{ shared: number }>('share', { paths })).shared,
    trash: async (paths) => (await call<{ trashed: number }>('trash', { paths })).trashed,
    startForeground: async (title, text, progress) => {
      await call('start_foreground', { title, text, progress })
    },
    stopForeground: async () => {
      await call('stop_foreground')
    },
    keepAwake: async (on) => {
      await call('keep_awake', { on })
    },
    publishExports: (paths) => call<{ published: number; uris: string[] }>('publish_exports', { paths }),
  }
}

/**
 * Permission flow for the album picker: ask (the system dialog is skipped when already
 * granted), then list. `denied` lets the UI show a "grant access in settings" hint.
 */
export async function loadAlbums(
  media: MediaApi,
): Promise<{ status: 'ok' | 'partial' | 'denied'; albums: Album[] }> {
  const perm = await media.requestPermission()
  if (!perm.granted) return { status: 'denied', albums: [] }
  return { status: perm.partial ? 'partial' : 'ok', albums: await media.listAlbums() }
}
