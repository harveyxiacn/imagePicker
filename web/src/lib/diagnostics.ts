/** The diagnostics bundle (P1-13): `POST /api/system/diagnostics` answers with a zip download. */

const FALLBACK_NAME = 'imagepicker-diagnostics.zip'

/** File name from a `Content-Disposition: attachment; filename="..."` header; only a plain `.zip` base name is trusted. */
export function diagnosticsFilename(header: string | null | undefined): string {
  const m = /filename="([^"\\/]+\.zip)"/.exec(header ?? '')
  return m ? m[1] : FALLBACK_NAME
}

/** Hands `blob` to the browser / webview as a file download. */
export function saveBlob(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = filename
  a.style.display = 'none'
  document.body.appendChild(a)
  a.click()
  a.remove()
  // the download has started by the time the task queue drains
  setTimeout(() => URL.revokeObjectURL(url), 10_000)
}

/** What the bundle holds and what it never holds (shown in Settings; keep in step with `ip-core::diagnostics`). */
export const DIAGNOSTICS_INCLUDED = ['versions', 'system', 'settings', 'logs', 'crashes', 'catalog'] as const
export const DIAGNOSTICS_EXCLUDED = ['photos', 'faces', 'paths', 'database', 'secrets'] as const
