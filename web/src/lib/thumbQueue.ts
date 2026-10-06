import { renderPreview } from '@/api/client'
import type { PreviewBody } from '@/api/types'

/**
 * Small live thumbnails (preset cards): POST /api/render/preview at a tiny `long_edge`,
 * at most `LIMIT` requests at a time, memoised per key (object URLs live for the session, capped).
 */
const LIMIT = 3
const CAP = 160
const cache = new Map<string, string>()
const inflight = new Map<string, Promise<string>>()
const waiting: (() => void)[] = []
let active = 0

async function slot<T>(fn: () => Promise<T>): Promise<T> {
  if (active >= LIMIT) await new Promise<void>((r) => waiting.push(r))
  active++
  try {
    return await fn()
  } finally {
    active--
    waiting.shift()?.()
  }
}

export function cachedThumb(key: string): string | undefined {
  return cache.get(key)
}

export function renderThumb(key: string, body: PreviewBody): Promise<string> {
  const hit = cache.get(key)
  if (hit) return Promise.resolve(hit)
  const pending = inflight.get(key)
  if (pending) return pending
  const p = slot(async () => {
    const { blob } = await renderPreview(body)
    const url = URL.createObjectURL(blob)
    cache.set(key, url)
    if (cache.size > CAP) {
      const first = cache.keys().next().value
      if (first !== undefined) {
        URL.revokeObjectURL(cache.get(first)!)
        cache.delete(first)
      }
    }
    return url
  }).finally(() => inflight.delete(key))
  inflight.set(key, p)
  return p
}
