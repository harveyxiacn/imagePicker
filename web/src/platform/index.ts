/**
 * Platform adapter. The same SPA runs in a Tauri window and in a plain browser.
 *
 * In the desktop shell the SPA is still served by the embedded HTTP server (relative `/api`);
 * Tauri only adds native extras, reached through `window.__TAURI_INTERNALS__.invoke` (no
 * `@tauri-apps/api` dependency needed) and DOM CustomEvents dispatched by the Rust shell:
 *  - `ip-native-import` (detail: folder path) - menu "Import Folder…" or an OS folder drop
 *  - `ip-native-drag`   (detail: boolean)      - an OS drag is hovering over the window
 * In the browser, `pickFolder` is null and the server-side folder browser (/api/fs/*) is used.
 */
import { useEffect } from 'react'
import { albumCoverUrl, createMediaApi, isAndroidShell, type MediaApi } from './android'

export * from './android'

declare global {
  interface Window {
    __TAURI_INTERNALS__?: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> }
    /** Random per-launch token set by the desktop shell. TODO: forward once the server enforces auth. */
    __IP_SESSION_TOKEN__?: string
  }
}

export const isTauri = (): boolean => typeof window !== 'undefined' && !!window.__TAURI_INTERNALS__

const invoke = <T>(cmd: string, args?: Record<string, unknown>): Promise<T> =>
  window.__TAURI_INTERNALS__!.invoke(cmd, args) as Promise<T>

export interface Platform {
  kind: 'tauri' | 'browser'
  /** Folder drops from the OS only work with the desktop shell. */
  canDropFolders: boolean
  /** Native folder picker; null = unavailable, use the server-side browser. */
  pickFolder: (() => Promise<string | null>) | null
  /** "Reveal in Explorer/Finder"; null = unavailable (browser). */
  revealInFolder: ((path: string) => Promise<void>) | null
  /** Android shell only: permissions, albums, share, trash, foreground service, publish. */
  media: MediaApi | null
  /** Android-only flat aliases of `media` (optional so desktop/browser need none). */
  requestPermission?: MediaApi['requestPermission']
  listAlbums?: MediaApi['listAlbums']
  albumCoverUrl?: (album: { cover_path?: string | null }) => string | null
  share?: MediaApi['share']
  trash?: MediaApi['trash']
  publishExports?: MediaApi['publishExports']
  startForeground?: MediaApi['startForeground']
  stopForeground?: MediaApi['stopForeground']
  keepAwake?: MediaApi['keepAwake']
}

export function getPlatform(): Platform {
  if (isAndroidShell()) {
    // No native folder dialog or drag-and-drop on Android: the UI lists albums (`listAlbums`).
    const media = createMediaApi((cmd, args) => window.__TAURI_INTERNALS__!.invoke(cmd, args))
    return {
      kind: 'tauri',
      canDropFolders: false,
      pickFolder: null,
      revealInFolder: null,
      media,
      // Flat aliases feature-detected by web/src/lib/mobilePlatform.ts.
      requestPermission: media.requestPermission,
      listAlbums: media.listAlbums,
      albumCoverUrl,
      share: media.share,
      trash: media.trash,
      publishExports: media.publishExports,
      startForeground: media.startForeground,
      stopForeground: media.stopForeground,
      keepAwake: media.keepAwake,
    }
  }
  if (isTauri()) {
    return {
      kind: 'tauri',
      canDropFolders: true,
      pickFolder: () => invoke<string | null>('pick_folder'),
      revealInFolder: (path) => invoke<void>('reveal_in_folder', { path }),
      media: null,
    }
  }
  return { kind: 'browser', canDropFolders: false, pickFolder: null, revealInFolder: null, media: null }
}

/** Subscribes to native import requests (menu / OS folder drop). No-op in the browser. */
export function useNativeImport(onPath: (path: string) => void, onDrag?: (over: boolean) => void): void {
  useEffect(() => {
    if (!isTauri()) return
    const onImport = (e: Event) => onPath((e as CustomEvent<string>).detail)
    const onDragEv = (e: Event) => onDrag?.((e as CustomEvent<boolean>).detail)
    window.addEventListener('ip-native-import', onImport)
    window.addEventListener('ip-native-drag', onDragEv)
    // An import requested while another page was open: the shell navigated home and left it here.
    try {
      const pending = sessionStorage.getItem('ip-pending-import')
      if (pending) {
        sessionStorage.removeItem('ip-pending-import')
        onPath(pending)
      }
    } catch {
      /* storage unavailable */
    }
    return () => {
      window.removeEventListener('ip-native-import', onImport)
      window.removeEventListener('ip-native-drag', onDragEv)
    }
  }, [onPath, onDrag])
}
