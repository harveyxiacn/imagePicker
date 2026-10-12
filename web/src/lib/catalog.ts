/** Catalog integrity + backups (`GET /api/catalog`, docs/02 §8). */
import type { CatalogStatus } from '@/api/types'

/** Query key of `GET /api/catalog`. */
export const CATALOG_KEY = ['catalog'] as const
/** Automatic backups the server keeps (`backup::KEEP`). */
export const BACKUPS_KEPT = 7

/** Local date + time of a backup. */
export const formatWhen = (ms: number, lang: string): string => new Date(ms).toLocaleString(lang, { dateStyle: 'medium', timeStyle: 'short' })

/** The start-up check found the catalog unreadable (replaced by an empty one) or damaged: offer the backups. */
export const needsRecovery = (s: CatalogStatus | undefined): boolean => !!s && (s.state === 'recovery' || s.state === 'damaged')

/** Poll while the start-up `quick_check` is still running. */
export const catalogRefetch = (s: CatalogStatus | undefined): number | false => (s?.state === 'checking' ? 1500 : false)
