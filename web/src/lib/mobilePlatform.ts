/**
 * Mobile platform capabilities (docs/api-contract-m8.md section A) consumed through `getPlatform()`.
 *
 * `web/src/platform` is owned by another agent. This module only *feature-detects* optional methods on
 * the object it returns, so it keeps working whatever the adapter ends up exposing:
 *
 *   listAlbums?():          Promise<Album[]>                          plugin `list_albums` (albums array unwrapped)
 *   requestPermission?():   Promise<{ granted: boolean; partial?: boolean }>   plugin `request_permission`
 *   scanQr?():              Promise<string | null>                    native QR scanner; resolves the decoded text
 *   albumCoverUrl?(a):      string | null                             WebView-loadable URL for `cover_path`
 *
 * With none of them present (desktop, plain browser) the album list is unavailable and the UI falls back
 * to the folder browser. In mock mode (`pnpm dev:mock`) a fake album list / permission is served by MSW.
 */
import { getPlatform } from '@/platform'

export interface Album {
  id: string
  name: string
  /** Folder path usable with `POST /api/import`. */
  path: string
  count: number
  cover_path?: string | null
  /** Optional ready-to-use cover URL (mock / platform supplied). */
  cover_url?: string | null
  latest_ms?: number | null
}

export type MediaPermission = 'granted' | 'partial' | 'denied'

export interface PermissionResult {
  granted: boolean
  partial?: boolean
}

interface PlatformExtras {
  listAlbums?: () => Promise<Album[] | { albums: Album[] }>
  requestPermission?: () => Promise<PermissionResult>
  scanQr?: () => Promise<string | null>
  albumCoverUrl?: (a: Album) => string | null
}

export interface MobilePlatform {
  /** `null` = no album access here (use the folder browser). */
  listAlbums: (() => Promise<Album[]>) | null
  /** `retry` = the user pressed "try again" after a denial. */
  requestPermission: ((retry?: boolean) => Promise<MediaPermission>) | null
  /** `null` = no scanner: the UI offers manual URL + code entry. */
  scanQr: (() => Promise<string | null>) | null
  coverUrl: (a: Album) => string | null
}

export function permissionFrom(r: PermissionResult): MediaPermission {
  if (r.granted && r.partial) return 'partial'
  return r.granted ? 'granted' : 'denied'
}

async function mockJson<T>(path: string): Promise<T> {
  const res = await fetch(path)
  if (!res.ok) throw new Error(`${res.status}`)
  return (await res.json()) as T
}

/** `?mock_media=denied|partial|granted|none` picks the mock permission state (default granted); a retry grants it. */
function mockMedia(): string {
  try {
    if (sessionStorage.getItem('ip_mock_media_granted') === '1') return 'granted'
    return new URLSearchParams(window.location.search).get('mock_media') ?? 'granted'
  } catch {
    return 'granted'
  }
}

export function getMobilePlatform(): MobilePlatform {
  const p = getPlatform() as unknown as PlatformExtras
  const native = typeof p.listAlbums === 'function'
  const mock = !native && typeof __MOCK__ !== 'undefined' && __MOCK__ && mockMedia() !== 'none'

  const listAlbums = native
    ? async () => {
        const r = await p.listAlbums!()
        return Array.isArray(r) ? r : (r.albums ?? [])
      }
    : mock
      ? async () => (await mockJson<{ albums: Album[] }>(`/api/mock/media/albums?state=${mockMedia()}`)).albums
      : null

  const requestPermission =
    typeof p.requestPermission === 'function'
      ? async () => permissionFrom(await p.requestPermission!())
      : mock
        ? async (retry?: boolean) => {
            if (retry) {
              try {
                sessionStorage.setItem('ip_mock_media_granted', '1')
              } catch {
                /* private mode */
              }
            }
            return permissionFrom(await mockJson<PermissionResult>(`/api/mock/media/permission?state=${mockMedia()}`))
          }
        : null

  return {
    listAlbums,
    requestPermission,
    scanQr: typeof p.scanQr === 'function' ? () => p.scanQr!() : null,
    coverUrl: (a) => a.cover_url ?? (typeof p.albumCoverUrl === 'function' ? p.albumCoverUrl(a) : null),
  }
}
