/** XMP sidecar conflicts (docs/api-contract-m6.md C / E): toast with "sidecar wins" / "catalog wins" actions. */
import type { QueryClient } from '@tanstack/react-query'
import { api } from '@/api/client'
import type { ServerEvent } from '@/api/types'
import i18n from '@/i18n'
import { useToasts } from '@/stores/toasts'
import { findCachedPhotos, qk } from './cache'

type Conflict = Extract<ServerEvent, { type: 'xmp.conflict' }>

const t = (k: string, o?: Record<string, unknown>) => i18n.t(k, o) as string

/** `xmp:Rating`: -1 = rejected, 0 / null = unrated. */
export function ratingLabel(r: number | null | undefined, tr: (k: string, o?: Record<string, unknown>) => string = t): string {
  if (r === -1) return tr('xmp.rejected')
  if (r === null || r === undefined || r === 0) return tr('xmp.unrated')
  return tr('xmp.stars', { n: r })
}

/** Library session id from the current URL (`/s/:id/...`). */
export const sessionFromPath = (pathname: string): number | null => {
  const m = pathname.match(/^\/s\/(\d+)/)
  return m ? Number(m[1]) : null
}

/** direction: `read` = the sidecar wins (re-read it), `write` = the catalog wins (overwrite the sidecar). */
async function resolve(qc: QueryClient, sessionId: number, direction: 'read' | 'write'): Promise<void> {
  try {
    await api.xmpSync(sessionId, direction)
    void qc.invalidateQueries({ queryKey: qk.photosAll })
    useToasts.getState().push('success', t(direction === 'read' ? 'xmp.resolvedSidecar' : 'xmp.resolvedCatalog'), 3000)
  } catch (e) {
    useToasts.getState().push('error', e instanceof Error ? e.message : String(e), 6000)
  }
}

export function handleXmpConflict(qc: QueryClient, ev: Conflict): void {
  const photo = findCachedPhotos(qc, [ev.photo_id])[0]
  const sessionId = photo?.session_id ?? sessionFromPath(location.pathname)
  const text = t('xmp.conflict', {
    name: photo?.file_name ?? `#${ev.photo_id}`,
    sidecar: ratingLabel(ev.sidecar.rating),
    catalog: ratingLabel(ev.catalog.rating),
  })
  useToasts.getState().push('info', text, 15_000, sessionId === null ? undefined : [
    { label: t('xmp.useSidecar'), onClick: () => void resolve(qc, sessionId, 'read') },
    { label: t('xmp.useCatalog'), onClick: () => void resolve(qc, sessionId, 'write') },
  ])
}
