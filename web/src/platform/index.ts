/**
 * Platform adapter. The same SPA runs in a Tauri window and in a plain browser.
 * Native capabilities are added here later; for now both modes use the
 * server-side folder browser (/api/fs/*).
 */

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown
  }
}

export const isTauri = (): boolean => typeof window !== 'undefined' && !!window.__TAURI_INTERNALS__

export interface Platform {
  kind: 'tauri' | 'browser'
  /** Folder drops from the OS only work with the desktop shell. */
  canDropFolders: boolean
  /** Native folder picker; null = unavailable, use the server-side browser. */
  pickFolder: (() => Promise<string | null>) | null
}

export function getPlatform(): Platform {
  if (isTauri()) {
    // TODO(tauri): wire @tauri-apps/plugin-dialog `open({ directory: true })` here.
    return { kind: 'tauri', canDropFolders: true, pickFolder: null }
  }
  return { kind: 'browser', canDropFolders: false, pickFolder: null }
}
