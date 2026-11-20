import { QueryClient } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { api } from '@/api/client'
import type { Photo } from '@/api/types'
import { useToasts } from '@/stores/toasts'
import { editPhotos } from './actions'
import { qk, type PhotosData } from './cache'
import { buildPhotosQuery, DEFAULT_FILTER } from './filter'
import { makePhoto } from './fixtures'
import { useHistory } from './history'

const key = qk.photos(1, buildPhotosQuery(1, DEFAULT_FILTER))
const photos = (rating: number | null): PhotosData => ({
  photos: [makePhoto(1, { user_rating: rating }), makePhoto(2, { user_rating: null })],
  total: 2,
})
const rating = (qc: QueryClient, id = 1): Photo['user_rating'] =>
  qc.getQueryData<PhotosData>(key)!.photos.find((p) => p.id === id)!.user_rating

/** A promise settled from outside, to hold a request "on the wire". */
function deferred<T = void>() {
  let resolve!: (v: T) => void
  let reject!: (e: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

/** Let every queued microtask and the next macrotask run. */
const flush = () => new Promise<void>((r) => setTimeout(r, 0))

type Call = { body: { ids: number[]; user_rating?: number | null }; done: ReturnType<typeof deferred<{ updated: number }>> }

/** Replace api.patchPhotos by a stub that records each call and waits for the test to settle it. */
function stubPatch(): Call[] {
  const calls: Call[] = []
  vi.spyOn(api, 'patchPhotos').mockImplementation((body) => {
    const done = deferred<{ updated: number }>()
    calls.push({ body: body as Call['body'], done })
    return done.promise
  })
  return calls
}

beforeEach(() => {
  useHistory.getState().clear()
  useToasts.setState({ toasts: [] })
})
afterEach(() => vi.restoreAllMocks())

describe('editPhotos', () => {
  it('keeps a new rating when a list refetch that started before the edit resolves with old data', async () => {
    const qc = new QueryClient()
    qc.setQueryData(key, photos(null))
    const calls = stubPatch()

    const refetch = deferred<PhotosData>()
    const inFlight = qc.fetchQuery({ queryKey: key, queryFn: () => refetch.promise, staleTime: 0 }).catch(() => undefined)
    await flush()

    const edit = editPhotos(qc, [1], { user_rating: 5 }, 'rate')
    await flush()
    expect(rating(qc)).toBe(5)

    refetch.resolve(photos(null)) // the response was computed before the PATCH
    await inFlight
    await flush()
    expect(rating(qc)).toBe(5)

    calls[0].done.resolve({ updated: 1 })
    await edit
    expect(rating(qc)).toBe(5)
  })

  it('applies the PATCHes of one photo strictly in call order', async () => {
    const qc = new QueryClient()
    qc.setQueryData(key, photos(null))
    const calls = stubPatch()

    const all = [1, 2, 3].map((r) => editPhotos(qc, [1], { user_rating: r }, 'rate'))
    await flush()
    expect(calls.map((c) => c.body.user_rating)).toEqual([1]) // the others wait for the first response
    expect(rating(qc)).toBe(3) // while the cache already shows the latest value

    calls[0].done.resolve({ updated: 1 })
    await flush()
    expect(calls.map((c) => c.body.user_rating)).toEqual([1, 2])
    calls[1].done.resolve({ updated: 1 })
    await flush()
    calls[2].done.resolve({ updated: 1 })
    await Promise.all(all)
    expect(calls.map((c) => c.body.user_rating)).toEqual([1, 2, 3])
    expect(rating(qc)).toBe(3)
  })

  it('does not touch different photos in parallel', async () => {
    const qc = new QueryClient()
    qc.setQueryData(key, photos(null))
    const calls = stubPatch()
    const a = editPhotos(qc, [1], { user_rating: 4 }, 'rate')
    const b = editPhotos(qc, [2], { user_rating: 2 }, 'rate')
    await flush()
    expect(calls).toHaveLength(2)
    calls.forEach((c) => c.done.resolve({ updated: 1 }))
    await Promise.all([a, b])
  })

  it('a failed first edit does not roll back over the newer value of the second edit', async () => {
    const qc = new QueryClient()
    qc.setQueryData(key, photos(null))
    const calls = stubPatch()

    const first = editPhotos(qc, [1], { user_rating: 3 }, 'rate')
    const second = editPhotos(qc, [1], { user_rating: 4 }, 'rate')
    await flush()
    expect(rating(qc)).toBe(4)

    calls[0].done.reject(new Error('first failed'))
    await first
    expect(rating(qc)).toBe(4)

    await flush()
    calls[1].done.resolve({ updated: 1 })
    await second
    expect(rating(qc)).toBe(4)
    expect(useToasts.getState().toasts.map((t) => t.kind)).toEqual(['error'])
  })

  it('a plain failure rolls the value back and shows the error', async () => {
    const qc = new QueryClient()
    qc.setQueryData(key, photos(2))
    const calls = stubPatch()

    const edit = editPhotos(qc, [1], { user_rating: 5 }, 'rate')
    await flush()
    expect(rating(qc)).toBe(5)
    calls[0].done.reject(new Error('boom'))
    await edit

    expect(rating(qc)).toBe(2)
    const toasts = useToasts.getState().toasts
    expect(toasts).toHaveLength(1)
    expect(toasts[0].kind).toBe('error')
    expect(toasts[0].text).toContain('boom')
  })
})
