import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { errorParts } from '@/lib/errors'
import { api, ApiError, authHooks, fetchAllPhotos, fetchSearchPhotos, isAbortError, PAGE_SIZE } from './client'
import { makePhoto } from '@/lib/fixtures'

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } })

const page = (ids: number[], next_cursor?: string, total = 10) => ({ photos: ids.map((id) => makePhoto(id)), total, next_cursor })

let fetchMock: ReturnType<typeof vi.fn>

beforeEach(() => {
  fetchMock = vi.fn()
  vi.stubGlobal('fetch', fetchMock)
})

afterEach(() => {
  vi.unstubAllGlobals()
  authHooks.onUnauthorized = null
})

const urlOf = (call: number) => new URL(String(fetchMock.mock.calls[call][0]), 'http://x')

describe('fetchAllPhotos', () => {
  it('pages until next_cursor is empty, passing the cursor along', async () => {
    fetchMock
      .mockResolvedValueOnce(json(page([1, 2], 'c1')))
      .mockResolvedValueOnce(json(page([3, 4], 'c2')))
      .mockResolvedValueOnce(json(page([5], undefined, 5)))
    const r = await fetchAllPhotos({ session_id: 7 })
    expect(r.photos.map((p) => p.id)).toEqual([1, 2, 3, 4, 5])
    expect(r.total).toBe(5)
    expect(fetchMock).toHaveBeenCalledTimes(3)
    expect(urlOf(0).searchParams.get('cursor')).toBeNull()
    expect(urlOf(0).searchParams.get('limit')).toBe(String(PAGE_SIZE))
    expect(urlOf(1).searchParams.get('cursor')).toBe('c1')
    expect(urlOf(2).searchParams.get('cursor')).toBe('c2')
  })

  it('treats an empty-string next_cursor as the end', async () => {
    fetchMock.mockResolvedValueOnce(json({ photos: [], total: 0, next_cursor: '' }))
    expect(await fetchAllPhotos({ session_id: 1 })).toEqual({ photos: [], total: 0 })
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })

  it('stops paging once the signal aborts between pages', async () => {
    const ac = new AbortController()
    fetchMock.mockImplementation(async () => {
      ac.abort() // the user navigates away while page 1 is arriving
      return json(page([1], 'c1'))
    })
    const err = await fetchAllPhotos({ session_id: 1 }, ac.signal).catch((e: unknown) => e)
    expect(isAbortError(err)).toBe(true)
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })

  it('hands the signal to fetch and rejects with an abort error when a request is cancelled', async () => {
    const ac = new AbortController()
    fetchMock.mockImplementation(
      (_url: string, init: RequestInit) =>
        new Promise((_res, rej) => {
          init.signal?.addEventListener('abort', () => rej(new DOMException('aborted', 'AbortError')))
        }),
    )
    const p = fetchAllPhotos({ session_id: 1 }, ac.signal).catch((e: unknown) => e)
    ac.abort()
    expect(isAbortError(await p)).toBe(true)
    expect(fetchMock.mock.calls[0][1].signal).toBe(ac.signal)
  })

  it('does not start at all when the signal is already aborted', async () => {
    const ac = new AbortController()
    ac.abort()
    await expect(fetchAllPhotos({ session_id: 1 }, ac.signal)).rejects.toSatisfy(isAbortError)
    expect(fetchMock).not.toHaveBeenCalled()
  })
})

describe('fetchSearchPhotos', () => {
  const hits = { hits: [{ photo_id: 2 }, { photo_id: 1 }], total_matched: 2, searched: 5, unindexed: 0 }

  it('ranks by /search then fetches the hits by id, forwarding the signal to both calls', async () => {
    const ac = new AbortController()
    fetchMock.mockResolvedValueOnce(json(hits)).mockResolvedValueOnce(json(page([1, 2], undefined, 2)))
    const r = await fetchSearchPhotos({ session_id: 1, search: 'dog' }, ac.signal)
    expect(r.photos.map((p) => p.id)).toEqual([2, 1])
    expect(fetchMock.mock.calls.map((c) => c[1].signal)).toEqual([ac.signal, ac.signal])
  })

  it('stops before the by-id fetch when aborted after /search answered', async () => {
    const ac = new AbortController()
    fetchMock.mockImplementation(async () => {
      ac.abort()
      return json(hits)
    })
    await expect(fetchSearchPhotos({ session_id: 1, search: 'dog' }, ac.signal)).rejects.toSatisfy(isAbortError)
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })
})

describe('request()', () => {
  it('calls the unauthorized hook with the request path on 401 and throws ApiError', async () => {
    const hook = vi.fn()
    authHooks.onUnauthorized = hook
    fetchMock.mockResolvedValueOnce(json({ error: { code: 'unauthorized', message: 'no' } }, 401))
    const err = await api.sessions().catch((e: unknown) => e)
    expect(err).toBeInstanceOf(ApiError)
    expect(err).toMatchObject({ status: 401, code: 'unauthorized' })
    expect(hook).toHaveBeenCalledExactlyOnceWith('/sessions')
  })

  it('maps a non-JSON error body to http_error with the status line', async () => {
    fetchMock.mockResolvedValueOnce(new Response('<html>Bad gateway</html>', { status: 502, statusText: 'Bad Gateway' }))
    const err = await api.sessions().catch((e: unknown) => e)
    expect(err).toBeInstanceOf(ApiError)
    expect(err).toMatchObject({ status: 502, code: 'http_error' })
    expect((err as ApiError).message).toContain('502')
  })

  it('maps a non-JSON 200 (login redirect / SPA fallback) to invalid_response, not a SyntaxError', async () => {
    fetchMock.mockResolvedValueOnce(new Response('<!doctype html><html></html>', { status: 200, headers: { 'Content-Type': 'text/html' } }))
    const err = await api.sessions().catch((e: unknown) => e)
    expect(err).toBeInstanceOf(ApiError)
    expect(err).toMatchObject({ status: 200, code: 'invalid_response' })
    expect(errorParts(err).key).toBe('errors.invalidResponse')
  })

  it('returns undefined for 204', async () => {
    fetchMock.mockResolvedValueOnce(new Response(null, { status: 204 }))
    await expect(api.deleteSession(1)).resolves.toBeUndefined()
  })

  it('returns the parsed body on 200', async () => {
    fetchMock.mockResolvedValueOnce(json({ sessions: [] }))
    await expect(api.sessions()).resolves.toEqual({ sessions: [] })
  })

  it('lets an abort during body parsing through (it is not an invalid_response)', async () => {
    const ac = new AbortController()
    const res = new Response('{}', { status: 200 })
    vi.spyOn(res, 'json').mockImplementation(() => {
      ac.abort()
      return Promise.reject(new DOMException('aborted', 'AbortError'))
    })
    fetchMock.mockResolvedValueOnce(res)
    await expect(api.photos({ session_id: 1 }, ac.signal)).rejects.toSatisfy(isAbortError)
  })
})
